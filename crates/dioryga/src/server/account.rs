//! 個人設定（設計書16.1のE領域）。
//!
//! 表示（言語・表示モード）とパスワードの2画面を持つ（#120）。**性格の違う
//! 操作を分ける**——表示は選んで保存するだけだが、パスワードは現行の入力が要り、
//! 他の端末のセッションを切る（20.7）。
//!
//! 以前は`must_change_password` の利用者は、この画面を
//! 終えるまで他の画面へ進めない（20.6）。
//!
//! **強制変更のときはメニューの無い画面で出す。**他の画面へ進めないのに
//! メニューを並べると、押しても戻される項目が並ぶ。それ以外は個人設定の
//! 画面として、上部と左のメニューを出す（#122）。

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::app_user;
use sea_orm::Set;
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::auth::session;
use crate::error::{AppError, AppResult};
use crate::repository::{Actor, AuditedTx};
use crate::server::view::{render, Chrome, Locale, Theme};
use crate::server::AppState;

#[derive(Debug, Deserialize)]
pub struct PasswordChangeForm {
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

#[derive(askama::Template)]
#[template(path = "password_change.html")]
struct PasswordChangePage {
    locale: &'static str,
    theme: &'static str,
    app_name: String,
    t_title: String,
    t_lead: String,
    t_current: String,
    t_new: String,
    t_confirm: String,
    t_submit: String,
    csrf_token: String,
    error: Option<String>,
}

impl PasswordChangePage {
    fn new(locale: Locale, theme: &'static str, csrf_token: String, error: Option<String>) -> Self {
        let l = locale.as_str();
        Self {
            locale: l,
            theme,
            app_name: rust_i18n::t!("app.name", locale = l).to_string(),
            t_title: rust_i18n::t!("password_change.title", locale = l).to_string(),
            t_lead: rust_i18n::t!("password_change.lead", locale = l).to_string(),
            t_current: rust_i18n::t!("password_change.current", locale = l).to_string(),
            t_new: rust_i18n::t!("password_change.new", locale = l).to_string(),
            t_confirm: rust_i18n::t!("password_change.confirm", locale = l).to_string(),
            t_submit: rust_i18n::t!("password_change.submit", locale = l).to_string(),
            csrf_token,
            error,
        }
    }
}

/// 表示の設定（言語・表示モード、#120）。
#[derive(askama::Template)]
#[template(path = "account_display.html")]
struct DisplayPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_locale: String,
    t_locale_hint: String,
    t_theme: String,
    t_theme_hint: String,
    t_theme_system: String,
    t_theme_light: String,
    t_theme_dark: String,
    t_submit: String,
    t_saved: String,
    locale_value: String,
    theme_value: &'static str,
    saved: bool,
}

#[derive(Debug, Deserialize)]
pub struct DisplayForm {
    pub locale: String,
    pub theme: String,
}

fn 表示を描く(current: &CurrentUser, saved: bool) -> AppResult<Response> {
    let l = locale_of(&current.user).as_str();
    render(&DisplayPage {
        chrome: Chrome::account(&current.user, current.csrf_token.clone(), "display"),
        t_title: rust_i18n::t!("account.display", locale = l).to_string(),
        t_lead: rust_i18n::t!("account.display_lead", locale = l).to_string(),
        t_locale: rust_i18n::t!("account.locale", locale = l).to_string(),
        t_locale_hint: rust_i18n::t!("account.locale_hint", locale = l).to_string(),
        t_theme: rust_i18n::t!("account.theme", locale = l).to_string(),
        t_theme_hint: rust_i18n::t!("account.theme_hint", locale = l).to_string(),
        t_theme_system: rust_i18n::t!("account.theme_system", locale = l).to_string(),
        t_theme_light: rust_i18n::t!("account.theme_light", locale = l).to_string(),
        t_theme_dark: rust_i18n::t!("account.theme_dark", locale = l).to_string(),
        t_submit: rust_i18n::t!("common.save", locale = l).to_string(),
        t_saved: rust_i18n::t!("account.saved", locale = l).to_string(),
        // 保存済みの値を選択状態にする。読めない値は既定へ寄せる
        locale_value: locale_of(&current.user).as_str().to_owned(),
        theme_value: Theme::parse(&current.user.theme).as_str(),
        saved,
    })
}

pub async fn display(Extension(current): Extension<CurrentUser>) -> AppResult<Response> {
    表示を描く(&current, false)
}

/// 言語と表示モードを保存する。
///
/// **語彙外は既定へ倒さず拒否する**（設計書8.6、Q-21）——選択肢はサーバが
/// 描画しており、他の値が届くのは改竄か不具合しかない。
pub async fn update_display(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Form(form): Form<DisplayForm>,
) -> AppResult<Response> {
    if !LOCALES.contains(&form.locale.as_str()) || !THEMES.contains(&form.theme.as_str()) {
        return Err(AppError::Validation("language or theme".to_owned()));
    }

    let user = current.user.clone();
    let tx = AuditedTx::begin(&state.db, Actor::User(user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.locale = Set(form.locale.clone());
    active.theme = Set(form.theme.clone());
    active.updated_at = Set(Utc::now());
    tx.update(&user, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // **保存した値でその場から描く。**リダイレクトすると、変えた本人が
    // 切り替わったことを確かめられるまで1画面ぶん遅れる
    let mut 更新後 = current;
    更新後.user.locale = form.locale;
    更新後.user.theme = form.theme;
    表示を描く(&更新後, true)
}

/// 選べる値（設計書16.5、16.4）。
const LOCALES: &[&str] = &["ja", "en"];
const THEMES: &[&str] = &["system", "light", "dark"];

/// 個人設定の画面としてのパスワード変更（#122）。入力欄は強制変更と共有する。
#[derive(askama::Template)]
#[template(path = "account_password.html")]
struct AccountPasswordPage {
    chrome: Chrome,
    t_title: String,
    t_lead: String,
    t_current: String,
    t_new: String,
    t_confirm: String,
    t_submit: String,
    csrf_token: String,
    error: Option<String>,
}

/// 強制変更かどうかで画面を選んで描く。
fn 描く(current: &CurrentUser, error: Option<String>) -> AppResult<Response> {
    let page = PasswordChangePage::new(
        locale_of(&current.user),
        Theme::parse(&current.user.theme).attribute(),
        current.csrf_token.clone(),
        error,
    );
    if current.user.must_change_password {
        return render(&page);
    }
    render(&AccountPasswordPage {
        chrome: Chrome::account(&current.user, current.csrf_token.clone(), "password"),
        t_title: page.t_title,
        // 強制変更の「続けるには」は、自分で開いた個人設定には合わない
        t_lead: rust_i18n::t!(
            "password_change.account_lead",
            locale = locale_of(&current.user).as_str()
        )
        .to_string(),
        t_current: page.t_current,
        t_new: page.t_new,
        t_confirm: page.t_confirm,
        t_submit: page.t_submit,
        csrf_token: page.csrf_token,
        error: page.error,
    })
}

/// 利用者の表示言語。ログイン後は `USER.locale` に従う（設計書16.5）。
fn locale_of(user: &app_user::Model) -> Locale {
    Locale::parse(&user.locale)
}

pub async fn show(Extension(current): Extension<CurrentUser>) -> AppResult<Response> {
    描く(&current, None)
}

pub async fn submit(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    _headers: HeaderMap,
    Form(form): Form<PasswordChangeForm>,
) -> AppResult<Response> {
    let locale = locale_of(&current.user);
    let l = locale.as_str();
    let user = current.user.clone();

    let 再表示 = |message: String| 描く(&current, Some(message));

    if form.new_password != form.confirm_password {
        return 再表示(rust_i18n::t!("password_change.mismatch", locale = l).to_string());
    }

    // 現行パスワードの再入力を必須とする（設計書20.7）。
    // セッションを乗っ取られた場合にパスワードごと奪われるのを防ぐ。
    let verified = state
        .passwords
        .verify(&form.current_password, &user.password_hash)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    if verified.is_none() {
        return 再表示(rust_i18n::t!("password_change.current_invalid", locale = l).to_string());
    }

    if let Err(e) = state
        .passwords
        .check_policy(&form.new_password, &user.username, &user.name)
    {
        return 再表示(e.to_string());
    }

    let hash = state
        .passwords
        .hash(&form.new_password)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let now = Utc::now();

    let tx = AuditedTx::begin(&state.db, Actor::User(user.id))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let mut active: app_user::ActiveModel = user.clone().into();
    active.password_hash = Set(hash);
    active.must_change_password = Set(false);
    active.updated_at = Set(now);
    tx.update(&user, active)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 現在のセッションだけを残し、他をすべて失効させる（設計書20.7）。
    // 変更した本人はログインしたまま、他の端末や乗っ取られた分を切る。
    let _ = session::revoke_all_except(&state.db, user.id, Some(current.session_id), now).await;

    Ok(Redirect::to("/").into_response())
}
