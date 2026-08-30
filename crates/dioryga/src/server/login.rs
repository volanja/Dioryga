//! ログイン・ログアウト（設計書20.6）。

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Form};
use chrono::Utc;
use entity::app_user;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::net::SocketAddr;

use crate::auth::middleware::CurrentUser;
use crate::auth::rate_limit::{self, Decision};
use crate::auth::{cookie, session};
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Locale};
use crate::server::AppState;

#[derive(askama::Template)]
#[template(path = "login.html")]
struct LoginPage {
    locale: &'static str,
    app_name: String,
    t_title: String,
    t_lead: String,
    t_email: String,
    t_password: String,
    t_submit: String,
    error: Option<String>,
    email: String,
}

impl LoginPage {
    fn new(locale: Locale, error: Option<String>, email: String) -> Self {
        let l = locale.as_str();
        Self {
            locale: l,
            app_name: rust_i18n::t!("app.name", locale = l).to_string(),
            t_title: rust_i18n::t!("login.title", locale = l).to_string(),
            t_lead: rust_i18n::t!("login.lead", locale = l).to_string(),
            t_email: rust_i18n::t!("login.email", locale = l).to_string(),
            t_password: rust_i18n::t!("login.password", locale = l).to_string(),
            t_submit: rust_i18n::t!("login.submit", locale = l).to_string(),
            error,
            email,
        }
    }
}

/// ログイン失敗時に返す文言。
///
/// **メールアドレスが存在しない場合と、パスワードが誤っている場合とで同一にする**
/// （設計書20.6）。文言が違うと、アカウントの存在有無を判別できてしまう。
/// 無効化された利用者も同じ文言で拒否する。
const LOGIN_FAILED: &str = "メールアドレスまたはパスワードが正しくありません";

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
}

pub async fn show(headers: HeaderMap) -> AppResult<Response> {
    let locale = Locale::from_headers(&headers);
    render(&LoginPage::new(locale, None, String::new()))
}

pub async fn submit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    let locale = Locale::from_headers(&headers);
    let now = Utc::now();
    let ip = peer.ip().to_string();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    // レート制限。恒久ロックではなく一時停止（設計書20.6）
    if rate_limit::check(&state.db, &form.email, &ip, now)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        == Decision::Throttled
    {
        return render(&LoginPage::new(
            locale,
            Some("試行回数が上限に達しました。しばらく待ってから再度お試しください".to_owned()),
            form.email,
        ));
    }

    let found = app_user::Entity::find()
        .filter(app_user::Column::Email.eq(&form.email))
        .one(&state.db)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    // 利用者が存在しない場合も検証を行い、応答時間の差を消す（設計書20.6）。
    // これを省くと「存在しなければ即座に返る」ことから存在有無が判別できる。
    let Some(user) = found else {
        let _ = state.passwords.verify_dummy(&form.password).await;
        record(&state, &form.email, &ip, false, now).await?;
        return render(&LoginPage::new(
            locale,
            Some(LOGIN_FAILED.to_owned()),
            form.email,
        ));
    };

    // 無効化された利用者も、存在しない場合と同じ扱いにする
    if user.disabled_at.is_some() {
        let _ = state.passwords.verify_dummy(&form.password).await;
        record(&state, &form.email, &ip, false, now).await?;
        return render(&LoginPage::new(
            locale,
            Some(LOGIN_FAILED.to_owned()),
            form.email,
        ));
    }

    let verified = state
        .passwords
        .verify(&form.password, &user.password_hash)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let Some(verified) = verified else {
        record(&state, &form.email, &ip, false, now).await?;
        return render(&LoginPage::new(
            locale,
            Some(LOGIN_FAILED.to_owned()),
            form.email,
        ));
    };

    // パラメータが現行より弱ければ再ハッシュする（設計書20.2）。
    // 平文が手元にあるのはこの瞬間だけであり、他に実施する機会がない。
    if verified.needs_rehash {
        if let Ok(hash) = state.passwords.hash(&form.password).await {
            let mut active: app_user::ActiveModel = user.clone().into();
            active.password_hash = Set(hash);
            active.updated_at = Set(now);
            let _ = active.update(&state.db).await;
        }
    }

    // 最終ログイン時刻の更新
    let mut active: app_user::ActiveModel = user.clone().into();
    active.last_login_at = Set(Some(now));
    let _ = active.update(&state.db).await;

    record(&state, &form.email, &ip, true, now).await?;

    // ログイン成功時は必ず新しいセッションを発行する（セッション固定攻撃の防止）
    let (_, token) = session::create(
        &state.db,
        user.id,
        &ip,
        user_agent,
        &state.config.session,
        now,
    )
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let 遷移先 = if user.must_change_password {
        "/account/password"
    } else {
        "/"
    };

    Ok((
        StatusCode::SEE_OTHER,
        [
            (
                header::SET_COOKIE,
                cookie::build(&token, &state.config.session),
            ),
            (header::LOCATION, 遷移先.to_owned()),
        ],
    )
        .into_response())
}

pub async fn logout(
    State(state): State<AppState>,
    current: Option<Extension<CurrentUser>>,
) -> AppResult<Response> {
    if let Some(Extension(current)) = current {
        if let Ok(Some(model)) = entity::session::Entity::find_by_id(current.session_id)
            .one(&state.db)
            .await
        {
            let _ = session::revoke(&state.db, model, Utc::now()).await;
        }
    }

    Ok((
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, cookie::clear(&state.config.session)),
            (header::LOCATION, "/login".to_owned()),
        ],
    )
        .into_response())
}

async fn record(
    state: &AppState,
    email: &str,
    ip: &str,
    succeeded: bool,
    now: chrono::DateTime<Utc>,
) -> AppResult<()> {
    rate_limit::record(&state.db, email, ip, succeeded, now)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}
