//! 初回セットアップの画面（設計書20.8）。
//!
//! 画面そのもの（Askamaテンプレート）はP3-4で作る。ここでは動作と遷移を用意する。

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::setup;
use crate::error::{AppError, AppResult};
use crate::server::view::{render, Locale};
use crate::server::AppState;

#[derive(askama::Template)]
#[template(path = "setup.html")]
struct SetupPage {
    locale: &'static str,
    /// 初期設定はまだ利用者がいないため、OSに従う（#120）。
    theme: &'static str,
    app_name: String,
    t_title: String,
    t_lead: String,
    t_token: String,
    t_token_hint: String,
    t_name: String,
    t_username: String,
    t_username_hint: String,
    t_email: String,
    t_email_hint: String,
    t_password: String,
    t_submit: String,
    error: Option<String>,
}

impl SetupPage {
    fn new(locale: Locale, error: Option<String>) -> Self {
        let l = locale.as_str();
        Self {
            locale: l,
            theme: "",
            app_name: rust_i18n::t!("app.name", locale = l).to_string(),
            t_title: rust_i18n::t!("setup.title", locale = l).to_string(),
            t_lead: rust_i18n::t!("setup.lead", locale = l).to_string(),
            t_token: rust_i18n::t!("setup.token", locale = l).to_string(),
            t_token_hint: rust_i18n::t!("setup.token_hint", locale = l).to_string(),
            t_name: rust_i18n::t!("setup.name", locale = l).to_string(),
            t_username: rust_i18n::t!("setup.username", locale = l).to_string(),
            t_username_hint: rust_i18n::t!("setup.username_hint", locale = l).to_string(),
            t_email: rust_i18n::t!("setup.email", locale = l).to_string(),
            t_email_hint: rust_i18n::t!("setup.email_hint", locale = l).to_string(),
            t_password: rust_i18n::t!("setup.password", locale = l).to_string(),
            t_submit: rust_i18n::t!("setup.submit", locale = l).to_string(),
            error,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SetupForm {
    pub token: String,
    pub name: String,
    pub username: String,
    /// 任意（設計書20.1）。空欄なら送られないこともある
    #[serde(default)]
    pub email: String,
    pub password: String,
}

/// セットアップ画面を表示する。
///
/// セットアップが完了していれば404を返す。**以降この経路が開くことはない**
/// （利用者が0件に戻ることがないため）。
pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if !state.setup.is_pending().await {
        return Err(AppError::NotFound);
    }

    render(&SetupPage::new(Locale::from_headers(&headers), None))
}

/// 最初のSystem Adminを作成する。
pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SetupForm>,
) -> AppResult<Response> {
    let locale = Locale::from_headers(&headers);

    let 結果 = setup::create_first_admin(
        &state.db,
        &state.setup,
        &state.passwords,
        &form.token,
        &form.name,
        &form.username,
        Some(form.email.as_str()),
        &form.password,
    )
    .await;

    // 入力の誤りは画面に戻して伝える。トークンの誤りとパスワードの不備は
    // いずれも利用者が直せるものであり、そのために画面を再表示する。
    match 結果 {
        Ok(_) => Ok(Redirect::to("/login").into_response()),
        Err(setup::SetupError::AlreadyCompleted) => Err(AppError::NotFound),
        Err(e @ setup::SetupError::InvalidToken) => {
            render(&SetupPage::new(locale, Some(e.to_string())))
        }
        Err(setup::SetupError::Password(e)) => render(&SetupPage::new(locale, Some(e.to_string()))),
        Err(setup::SetupError::Username(e)) => render(&SetupPage::new(locale, Some(e.to_string()))),
        Err(other) => Err(AppError::Internal(anyhow::anyhow!(other))),
    }
}

/// セットアップが済むまで、他のURLを `/setup` へ誘導する。
///
/// 許可リスト方式にしている。**新しい画面を追加したときに誘導し忘れる事故**を
/// 防ぐため、明示した経路以外はすべて誘導の対象とする（設計書20.10と同じ考え方）。
pub async fn redirect_while_pending(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();

    // セットアップ自身・ヘルスチェック・静的アセットは通す。
    // ヘルスチェックを通すのは、CIのスモークテストが起動直後に叩くため（2章）。
    // 静的アセットを通すのは、セットアップ画面自身がCSSを読むため。
    let 通す = path == "/setup" || path == "/health" || path.starts_with("/assets/");

    if !通す && state.setup.is_pending().await {
        return Redirect::to("/setup").into_response();
    }

    next.run(request).await
}

/// セットアップ完了後に `/setup` が404を返すことの確認は、
/// 結合テスト（tests/setup.rs）で行う。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn フォームの項目が揃っている() {
        // 項目名はテンプレート（P3-4）と対応する必要がある
        let json = r#"{"token":"t","name":"n","username":"u","password":"p"}"#;
        let form: SetupForm = serde_json::from_str(json).unwrap();
        assert_eq!(form.token, "t");
        assert_eq!(form.username, "u");
        // メールアドレスは任意（設計書20.1）
        assert_eq!(form.email, "");
    }
}
