//! 画面（Askamaテンプレート）と静的アセット。
//!
//! # 翻訳の扱い
//!
//! 翻訳の対象はUIラベルのみで、利用者が入力したデータは対象外（設計書16.5）。
//! 表示言語は `USER.locale` に従うが、ログイン前は利用者が定まらないため、
//! ブラウザの `Accept-Language` から推測する。
//!
//! # 静的アセット
//!
//! `rust-embed` でバイナリへ埋め込む（設計書2章）。配布物を単一の実行ファイルに
//! 収めるため、ファイルを別途配置させない。

use askama::Template;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use crate::error::AppError;

/// 対応する表示言語（設計書16.5）。英語をベース言語とし、日本語に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Ja,
    En,
}

impl Locale {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }

    pub fn parse(value: &str) -> Self {
        if value.starts_with("ja") {
            Self::Ja
        } else {
            Self::En
        }
    }

    /// ブラウザの `Accept-Language` から推測する。
    ///
    /// 初回ログイン時の `USER.locale` の初期値にも同じ推測を使う（設計書5章）。
    pub fn from_headers(headers: &HeaderMap) -> Self {
        headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| Self::parse(v.trim()))
            .unwrap_or(Self::Ja)
    }
}

/// ログイン後の画面が共通で持つ枠（`app_layout.html`）。
///
/// **各ページの構造体にこれを1つ持たせる。**ヘッダーとナビに必要な値は
/// どの画面でも同じであり、画面を足すたびに同じフィールドを並べ直したくない。
pub struct Chrome {
    pub locale: &'static str,
    pub app_name: String,
    pub user_name: String,
    /// 現在選択中のナビ項目。`users` / `projects` / `account`。
    pub nav: &'static str,
    /// フォームのhidden fieldへ埋め込むCSRFトークン（設計書20.5）。
    pub csrf_token: String,
    pub is_system_admin: bool,
    pub t_logout: String,
    pub t_nav_users: String,
    pub t_nav_projects: String,
    pub t_nav_account: String,
}

impl Chrome {
    pub fn new(user: &entity::app_user::Model, csrf_token: String, nav: &'static str) -> Self {
        let locale = Locale::parse(&user.locale);
        let l = locale.as_str();
        Self {
            locale: l,
            app_name: rust_i18n::t!("app.name", locale = l).to_string(),
            user_name: user.name.clone(),
            nav,
            csrf_token,
            is_system_admin: user.is_system_admin,
            t_logout: rust_i18n::t!("common.logout", locale = l).to_string(),
            t_nav_users: rust_i18n::t!("nav.users", locale = l).to_string(),
            t_nav_projects: rust_i18n::t!("nav.projects", locale = l).to_string(),
            t_nav_account: rust_i18n::t!("nav.account", locale = l).to_string(),
        }
    }

    pub fn locale(&self) -> Locale {
        Locale::parse(self.locale)
    }
}

/// バイナリへ埋め込む静的アセット。
#[derive(rust_embed::Embed)]
#[folder = "assets/"]
struct Assets;

/// 静的アセットを返す。
pub async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let Some(file) = Assets::get(&path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref())],
        file.data.into_owned(),
    )
        .into_response()
}

/// テンプレートを描画する。
///
/// 描画の失敗はプログラムの誤りであり、利用者の入力では起こらない。
/// 内部エラーとして扱い、詳細は応答に含めない。
pub fn render<T: Template>(template: &T) -> Result<Response, AppError> {
    template
        .render()
        .map(|body| Html(body).into_response())
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_languageから言語を推測する() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_LANGUAGE, "ja,en-US;q=0.9".parse().unwrap());
        assert_eq!(Locale::from_headers(&headers), Locale::Ja);

        headers.insert(header::ACCEPT_LANGUAGE, "en-US,en;q=0.9".parse().unwrap());
        assert_eq!(Locale::from_headers(&headers), Locale::En);
    }

    #[test]
    fn accept_languageが無ければ日本語にする() {
        assert_eq!(Locale::from_headers(&HeaderMap::new()), Locale::Ja);
    }

    #[test]
    fn 静的アセットが埋め込まれている() {
        assert!(
            Assets::get("dioryga.css").is_some(),
            "CSSがバイナリに埋め込まれていません"
        );
    }
}
