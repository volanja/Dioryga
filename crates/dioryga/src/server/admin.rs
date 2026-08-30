//! System Admin領域のユーザー管理（設計書16.1のA領域、4章、20.7、20.11）。
//!
//! # 削除しない
//!
//! 利用者は物理削除せず、`disabled_at` による無効化のみを行う（20.11）。
//! `AUDIT_LOG.user_id` や各カタログの `created_by` から参照されており、
//! 消すと過去の記録が壊れるため。
//!
//! # ロックアウトの防止
//!
//! **自分自身を無効化・降格できない**ようにする。これが欠けると、画面の操作だけで
//! 誰もログインできない状態を作れてしまう（復旧はCLIの `dioryga admin create` に
//! 頼ることになり、DBへ到達できる者を呼ぶまで運用が止まる）。
//!
//! 「最後の有効なSystem Adminを守る」という別の条件は**置いていない。**この画面を
//! 操作できるのは有効なSystem Admin本人だけであり（`system_admin_only` と、
//! 無効化された利用者を通さない `authenticate`）、他人を無効化・降格しても操作者
//! 自身が残る。自分自身への操作は上の規則が止める。条件を足しても到達しない。

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::app_user;
use sea_orm::sea_query::{Expr, Func, LikeExpr};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, ExprTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::auth::password;
use crate::auth::session;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale};
use crate::server::AppState;

// ---------------------------------------------------------------------------
// 画面
// ---------------------------------------------------------------------------

struct UserRow {
    id: i32,
    name: String,
    email: String,
    role: String,
    last_login: String,
    status: String,
    disabled: bool,
}

/// 発行した一時パスワード。**この応答にしか現れない**（設計書20.7）。
struct Issued {
    email: String,
    password: String,
}

#[derive(askama::Template)]
#[template(path = "admin_users.html")]
struct UsersPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_new: String,
    t_keyword: String,
    t_search: String,
    t_status_all: String,
    t_status_active: String,
    t_status_disabled: String,
    t_name: String,
    t_email: String,
    t_role: String,
    t_last_login: String,
    t_status: String,
    t_actions: String,
    t_edit: String,
    t_enable: String,
    t_disable: String,
    t_reset_password: String,
    t_empty: String,
    t_issued: String,
    t_issued_hint: String,
    q: String,
    status: String,
    rows: Vec<UserRow>,
    error: Option<String>,
    issued: Option<Issued>,
}

#[derive(askama::Template)]
#[template(path = "admin_user_form.html")]
struct UserFormPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_back: String,
    t_name: String,
    t_email: String,
    t_locale: String,
    t_is_system_admin: String,
    t_system_admin_hint: String,
    t_submit: String,
    /// 送信先。新規と編集でテンプレートを共用するために持たせる。
    action: String,
    name: String,
    email: String,
    locale_value: String,
    is_system_admin: bool,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// 一覧
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// 絞り込みの状態。既定は「有効」。
///
/// 16.1の通り、**完了済み・無効化済みを隠せるフィルタは必須**とする。
/// 既定を「すべて」にすると、退職者が並ぶ一覧を毎回目で除くことになる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Active,
    Disabled,
    All,
}

impl StatusFilter {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("disabled") => Self::Disabled,
            Some("all") => Self::All,
            _ => Self::Active,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::All => "all",
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let page = build_list(&state, &current, &query, None, None).await?;
    render(&page)
}

/// 一覧画面を組み立てる。
///
/// 一時パスワードの発行結果や誤操作の警告も、リダイレクトを挟まずこの画面に
/// 載せて返す。**一時パスワードをリダイレクト後に見せるには、平文をどこかに
/// 預ける必要がある**（フラッシュメッセージ用の記憶域）。20.7の「平文を保持
/// しない」という要求と噛み合わないため、そのまま描画して返す。
async fn build_list(
    state: &AppState,
    current: &CurrentUser,
    query: &ListQuery,
    error: Option<String>,
    issued: Option<Issued>,
) -> AppResult<UsersPage> {
    let locale = Locale::parse(&current.user.locale);
    let l = locale.as_str();
    let status = StatusFilter::parse(query.status.as_deref());
    let keyword = query.q.clone().unwrap_or_default();

    let users = 検索(&state.db, &keyword, status)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let rows = users
        .into_iter()
        .map(|user| UserRow {
            role: if user.is_system_admin {
                rust_i18n::t!("users.role_system_admin", locale = l).to_string()
            } else {
                rust_i18n::t!("users.role_member", locale = l).to_string()
            },
            last_login: match user.last_login_at {
                // 時刻はUTCのまま扱う（設計書24.3）。表示にもUTCであることを添える
                Some(at) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
                None => rust_i18n::t!("users.never", locale = l).to_string(),
            },
            status: if user.disabled_at.is_some() {
                rust_i18n::t!("users.status_disabled", locale = l).to_string()
            } else {
                rust_i18n::t!("users.status_active", locale = l).to_string()
            },
            disabled: user.disabled_at.is_some(),
            id: user.id,
            name: user.name,
            email: user.email,
        })
        .collect();

    Ok(UsersPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "users"),
        t_title: rust_i18n::t!("users.title", locale = l).to_string(),
        t_lead: rust_i18n::t!("users.lead", locale = l).to_string(),
        t_new: rust_i18n::t!("users.new", locale = l).to_string(),
        t_keyword: rust_i18n::t!("common.keyword", locale = l).to_string(),
        t_search: rust_i18n::t!("common.search", locale = l).to_string(),
        t_status_all: rust_i18n::t!("users.status_all", locale = l).to_string(),
        t_status_active: rust_i18n::t!("users.status_active", locale = l).to_string(),
        t_status_disabled: rust_i18n::t!("users.status_disabled", locale = l).to_string(),
        t_name: rust_i18n::t!("users.name", locale = l).to_string(),
        t_email: rust_i18n::t!("users.email", locale = l).to_string(),
        t_role: rust_i18n::t!("users.role", locale = l).to_string(),
        t_last_login: rust_i18n::t!("users.last_login", locale = l).to_string(),
        t_status: rust_i18n::t!("users.status", locale = l).to_string(),
        t_actions: rust_i18n::t!("users.actions", locale = l).to_string(),
        t_edit: rust_i18n::t!("users.edit", locale = l).to_string(),
        t_enable: rust_i18n::t!("users.enable", locale = l).to_string(),
        t_disable: rust_i18n::t!("users.disable", locale = l).to_string(),
        t_reset_password: rust_i18n::t!("users.reset_password", locale = l).to_string(),
        t_empty: rust_i18n::t!("users.empty", locale = l).to_string(),
        t_issued: rust_i18n::t!("users.issued", locale = l).to_string(),
        t_issued_hint: rust_i18n::t!("users.issued_hint", locale = l).to_string(),
        q: keyword,
        status: status.as_str().to_owned(),
        rows,
        error,
        issued,
    })
}

/// 表示名またはメールアドレスの部分一致で絞り込む。
///
/// **`LOWER()` を両側に掛けている。**PostgreSQLの `LIKE` は大文字小文字を区別し、
/// SQLiteのそれはASCIIに限り区別しない。素directに書くとDBによって結果が変わる。
async fn 検索<C: ConnectionTrait>(
    db: &C,
    keyword: &str,
    status: StatusFilter,
) -> Result<Vec<app_user::Model>, sea_orm::DbErr> {
    let mut query = app_user::Entity::find();

    match status {
        StatusFilter::Active => query = query.filter(app_user::Column::DisabledAt.is_null()),
        StatusFilter::Disabled => query = query.filter(app_user::Column::DisabledAt.is_not_null()),
        StatusFilter::All => {}
    }

    let keyword = keyword.trim();
    if !keyword.is_empty() {
        let pattern = format!("%{}%", escape_like(&keyword.to_lowercase()));
        query = query.filter(
            Expr::expr(Func::lower(Expr::col(app_user::Column::Name)))
                .like(LikeExpr::new(&pattern).escape('\\'))
                .or(Expr::expr(Func::lower(Expr::col(app_user::Column::Email)))
                    .like(LikeExpr::new(&pattern).escape('\\'))),
        );
    }

    query
        .order_by_asc(app_user::Column::Name)
        .order_by_asc(app_user::Column::Id)
        .all(db)
        .await
}

/// `LIKE` のワイルドカードを打ち消す。
///
/// 打ち消さないと、利用者が `%` と入力しただけで全件一致になる。
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ---------------------------------------------------------------------------
// 登録・編集
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UserForm {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub locale: Option<String>,
    /// チェックボックスは**チェックされたときだけ**送られる。
    /// 未チェックを表す値は届かないため、`Option` で受ける。
    #[serde(default)]
    pub is_system_admin: Option<String>,
}

impl UserForm {
    fn is_system_admin(&self) -> bool {
        self.is_system_admin.is_some()
    }

    fn locale(&self) -> String {
        match self.locale.as_deref() {
            Some("en") => "en".to_owned(),
            _ => "ja".to_owned(),
        }
    }
}

pub async fn new_form(Extension(current): Extension<CurrentUser>) -> AppResult<Response> {
    render(&登録画面(
        &current,
        String::new(),
        String::new(),
        false,
        None,
    ))
}

fn 登録画面(
    current: &CurrentUser,
    name: String,
    email: String,
    is_system_admin: bool,
    error: Option<String>,
) -> UserFormPage {
    let l = Locale::parse(&current.user.locale).as_str();
    UserFormPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "users"),
        t_title: rust_i18n::t!("users.new_title", locale = l).to_string(),
        t_lead: rust_i18n::t!("users.new_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("common.back", locale = l).to_string(),
        t_name: rust_i18n::t!("users.name", locale = l).to_string(),
        t_email: rust_i18n::t!("users.email", locale = l).to_string(),
        t_locale: rust_i18n::t!("users.locale", locale = l).to_string(),
        t_is_system_admin: rust_i18n::t!("users.is_system_admin", locale = l).to_string(),
        t_system_admin_hint: rust_i18n::t!("users.system_admin_hint", locale = l).to_string(),
        t_submit: rust_i18n::t!("common.create", locale = l).to_string(),
        action: "/admin/users".to_owned(),
        name,
        email,
        locale_value: "ja".to_owned(),
        is_system_admin,
        error,
    }
}

/// 利用者を作成する。
///
/// パスワードはSystem Adminが決めるのではなく、**一時パスワードを発行して一度だけ
/// 表示する**（設計書20.7）。System Adminが決めた値をそのまま使い続けられると、
/// 本人以外がパスワードを知っている状態が残る。
pub async fn create(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<UserForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let name = form.name.trim().to_owned();
    let email = form.email.trim().to_lowercase();

    let 再表示 = |message: String| {
        登録画面(
            &current,
            name.clone(),
            email.clone(),
            form.is_system_admin(),
            Some(message),
        )
    };

    if name.is_empty() {
        return render(&再表示(
            rust_i18n::t!("users.name_required", locale = l).to_string(),
        ));
    }
    if email.is_empty() {
        return render(&再表示(
            rust_i18n::t!("users.email_required", locale = l).to_string(),
        ));
    }

    let 既存 = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(&email))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    if 既存.is_some() {
        return render(&再表示(
            rust_i18n::t!("users.duplicate_email", locale = l).to_string(),
        ));
    }

    let temporary = password::generate_temporary().map_err(|e| AppError::Internal(e.into()))?;
    let hash = state
        .passwords
        .hash(&temporary)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.insert(app_user::ActiveModel {
        name: Set(name),
        email: Set(email.clone()),
        password_hash: Set(hash),
        // 本人が最初のログインで自分のパスワードに変える（設計書20.6）
        must_change_password: Set(true),
        is_system_admin: Set(form.is_system_admin()),
        locale: Set(form.locale()),
        last_login_at: Set(None),
        disabled_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let page = build_list(
        &state,
        &current,
        &ListQuery::default(),
        None,
        Some(Issued {
            email,
            password: temporary,
        }),
    )
    .await?;
    render(&page)
}

pub async fn edit_form(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;
    render(&編集画面(&current, &target, None))
}

fn 編集画面(
    current: &CurrentUser,
    target: &app_user::Model,
    error: Option<String>,
) -> UserFormPage {
    let l = Locale::parse(&current.user.locale).as_str();
    UserFormPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "users"),
        t_title: rust_i18n::t!("users.edit_title", locale = l).to_string(),
        t_lead: rust_i18n::t!("users.edit_lead", locale = l).to_string(),
        t_back: rust_i18n::t!("common.back", locale = l).to_string(),
        t_name: rust_i18n::t!("users.name", locale = l).to_string(),
        t_email: rust_i18n::t!("users.email", locale = l).to_string(),
        t_locale: rust_i18n::t!("users.locale", locale = l).to_string(),
        t_is_system_admin: rust_i18n::t!("users.is_system_admin", locale = l).to_string(),
        t_system_admin_hint: rust_i18n::t!("users.system_admin_hint", locale = l).to_string(),
        t_submit: rust_i18n::t!("common.save", locale = l).to_string(),
        action: format!("/admin/users/{}", target.id),
        name: target.name.clone(),
        email: target.email.clone(),
        locale_value: target.locale.clone(),
        is_system_admin: target.is_system_admin,
        error,
    }
}

pub async fn update(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
    Form(form): Form<UserForm>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, id).await?;

    let name = form.name.trim().to_owned();
    let email = form.email.trim().to_lowercase();

    if name.is_empty() {
        return render(&編集画面(
            &current,
            &target,
            Some(rust_i18n::t!("users.name_required", locale = l).to_string()),
        ));
    }
    if email.is_empty() {
        return render(&編集画面(
            &current,
            &target,
            Some(rust_i18n::t!("users.email_required", locale = l).to_string()),
        ));
    }

    if email != target.email {
        let 衝突 = app_user::Entity::find()
            .filter(app_user::Column::Email.eq(&email))
            .one(&state.db)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        if 衝突.is_some() {
            return render(&編集画面(
                &current,
                &target,
                Some(rust_i18n::t!("users.duplicate_email", locale = l).to_string()),
            ));
        }
    }

    // 自分自身のSystem Admin権限は外せない（ロックアウトの防止）
    if target.id == current.user.id && target.is_system_admin && !form.is_system_admin() {
        return render(&編集画面(
            &current,
            &target,
            Some(rust_i18n::t!("users.cannot_demote_self", locale = l).to_string()),
        ));
    }

    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut active: app_user::ActiveModel = target.clone().into();
    active.name = Set(name);
    active.email = Set(email);
    active.locale = Set(form.locale());
    active.is_system_admin = Set(form.is_system_admin());
    active.updated_at = Set(Utc::now());
    tx.update(&target, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    Ok(Redirect::to("/admin/users").into_response())
}

// ---------------------------------------------------------------------------
// 無効化・再有効化・パスワードリセット
// ---------------------------------------------------------------------------

/// 利用者を無効化する（設計書20.11）。
///
/// **物理削除はしない。**あわせて既存のセッションをすべて失効させる。
/// これを省くと、無効化しても発行済みのセッションが生きている間は操作できる。
pub async fn disable(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let l = Locale::parse(&current.user.locale).as_str();
    let target = 対象(&state, id).await?;

    if target.id == current.user.id {
        return 一覧へ戻す(
            &state,
            &current,
            rust_i18n::t!("users.cannot_disable_self", locale = l).to_string(),
        )
        .await;
    }

    let now = Utc::now();
    if target.disabled_at.is_none() {
        変更(&state, &current, &target, |active| {
            active.disabled_at = Set(Some(now));
        })
        .await?;

        session::revoke_all_except(&state.db, target.id, None, now)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }

    Ok(Redirect::to("/admin/users").into_response())
}

pub async fn enable(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;

    if target.disabled_at.is_some() {
        変更(&state, &current, &target, |active| {
            active.disabled_at = Set(None);
        })
        .await?;
    }

    Ok(Redirect::to("/admin/users").into_response())
}

/// 一時パスワードを発行し直す（設計書20.7）。
pub async fn reset_password(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i32>,
) -> AppResult<Response> {
    let target = 対象(&state, id).await?;

    let temporary = password::generate_temporary().map_err(|e| AppError::Internal(e.into()))?;
    let hash = state
        .passwords
        .hash(&temporary)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    変更(&state, &current, &target, |active| {
        active.password_hash = Set(hash);
        active.must_change_password = Set(true);
    })
    .await?;

    // 既存セッションをすべて失効させる（設計書20.7）
    session::revoke_all_except(&state.db, target.id, None, now)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let page = build_list(
        &state,
        &current,
        &ListQuery::default(),
        None,
        Some(Issued {
            email: target.email,
            password: temporary,
        }),
    )
    .await?;
    render(&page)
}

// ---------------------------------------------------------------------------
// 補助
// ---------------------------------------------------------------------------

async fn 対象(state: &AppState, id: i32) -> AppResult<app_user::Model> {
    app_user::Entity::find_by_id(id)
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .ok_or(AppError::NotFound)
}

/// 監査ログを伴って1件更新する。
async fn 変更<F>(
    state: &AppState,
    current: &CurrentUser,
    target: &app_user::Model,
    適用: F,
) -> AppResult<()>
where
    F: FnOnce(&mut app_user::ActiveModel),
{
    let tx = AuditedTx::begin(&state.db, Actor::User(current.user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let mut active: app_user::ActiveModel = target.clone().into();
    適用(&mut active);
    active.updated_at = Set(Utc::now());

    tx.update(target, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

async fn 一覧へ戻す(
    state: &AppState,
    current: &CurrentUser,
    message: String,
) -> AppResult<Response> {
    let page = build_list(state, current, &ListQuery::default(), Some(message), None).await?;
    render(&page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 状態フィルタの既定は有効のみ() {
        assert_eq!(StatusFilter::parse(None), StatusFilter::Active);
        assert_eq!(StatusFilter::parse(Some("")), StatusFilter::Active);
        assert_eq!(StatusFilter::parse(Some("all")), StatusFilter::All);
        assert_eq!(
            StatusFilter::parse(Some("disabled")),
            StatusFilter::Disabled
        );
    }

    #[test]
    fn likeのワイルドカードが打ち消される() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c\\d"), "c\\\\d");
    }

    #[test]
    fn チェックされないチェックボックスは偽になる() {
        let 未チェック: UserForm =
            serde_urlencoded::from_str("name=n&email=e@example.com").unwrap();
        assert!(!未チェック.is_system_admin());

        let チェック済: UserForm =
            serde_urlencoded::from_str("name=n&email=e@example.com&is_system_admin=on").unwrap();
        assert!(チェック済.is_system_admin());
    }

    #[test]
    fn 未知のlocaleは日本語に倒す() {
        let form: UserForm =
            serde_urlencoded::from_str("name=n&email=e@example.com&locale=fr").unwrap();
        assert_eq!(form.locale(), "ja");
    }
}
