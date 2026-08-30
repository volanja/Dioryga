//! ログイン・ログアウト（設計書20.6）。

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
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
use crate::server::AppState;

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

pub async fn show() -> Html<&'static str> {
    // 画面はP3-4で実装する。
    Html(
        "<!doctype html><meta charset=\"utf-8\"><title>ログイン</title>\
         <p>ログイン。画面はP3-4で実装する。</p>",
    )
}

pub async fn submit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
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
        return Err(AppError::Validation(
            "試行回数が上限に達しました。しばらく待ってから再度お試しください".to_owned(),
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
        return Err(AppError::Validation(LOGIN_FAILED.to_owned()));
    };

    // 無効化された利用者も、存在しない場合と同じ扱いにする
    if user.disabled_at.is_some() {
        let _ = state.passwords.verify_dummy(&form.password).await;
        record(&state, &form.email, &ip, false, now).await?;
        return Err(AppError::Validation(LOGIN_FAILED.to_owned()));
    }

    let verified = state
        .passwords
        .verify(&form.password, &user.password_hash)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let Some(verified) = verified else {
        record(&state, &form.email, &ip, false, now).await?;
        return Err(AppError::Validation(LOGIN_FAILED.to_owned()));
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
