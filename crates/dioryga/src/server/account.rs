//! 個人設定（設計書16.1のE領域）。
//!
//! 現時点ではパスワード変更のみ。`must_change_password` の利用者は、この画面を
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
use crate::server::view::{render, Chrome, Locale};
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
    fn new(locale: Locale, csrf_token: String, error: Option<String>) -> Self {
        let l = locale.as_str();
        Self {
            locale: l,
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
    let page = PasswordChangePage::new(locale_of(&current.user), current.csrf_token.clone(), error);
    if current.user.must_change_password {
        return render(&page);
    }
    render(&AccountPasswordPage {
        chrome: Chrome::new(&current.user, current.csrf_token.clone(), "account"),
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
