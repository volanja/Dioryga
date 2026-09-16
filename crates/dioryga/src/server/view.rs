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
    /// 現在選択中のナビ項目。`users` / `projects` / `warehouses` / `catalog` / `account`。
    pub nav: &'static str,
    /// 共有カタログの下位メニューで、現在の画面にあたる項目（#118）。
    /// `vendors` / `chassis_models` / `parts` / `configurations` / `cables` /
    /// `software` / `vlans` / `merge`。ダッシュボードとカタログ以外では空。
    ///
    /// **詳細画面では親の一覧の項目を指す。**どこにいるか分かることが目的であり、
    /// 詳細に入った途端にメニューの印が消えると迷う。
    pub sub: &'static str,
    /// フォームのhidden fieldへ埋め込むCSRFトークン（設計書20.5）。
    pub csrf_token: String,
    pub is_system_admin: bool,
    pub t_logout: String,
    pub t_nav_users: String,
    pub t_nav_projects: String,
    pub t_nav_warehouses: String,
    pub t_nav_catalog: String,
    pub t_nav_vendors: String,
    pub t_nav_chassis_models: String,
    pub t_nav_parts: String,
    pub t_nav_configurations: String,
    pub t_nav_cables: String,
    pub t_nav_software: String,
    pub t_nav_vlans: String,
    pub t_nav_merge: String,
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
            sub: "",
            csrf_token,
            is_system_admin: user.is_system_admin,
            t_logout: rust_i18n::t!("common.logout", locale = l).to_string(),
            t_nav_users: rust_i18n::t!("nav.users", locale = l).to_string(),
            t_nav_projects: rust_i18n::t!("nav.projects", locale = l).to_string(),
            t_nav_warehouses: rust_i18n::t!("warehouses.title", locale = l).to_string(),
            t_nav_catalog: rust_i18n::t!("catalog.nav", locale = l).to_string(),
            t_nav_vendors: rust_i18n::t!("catalog.vendors", locale = l).to_string(),
            t_nav_chassis_models: rust_i18n::t!("catalog.chassis_models", locale = l).to_string(),
            t_nav_parts: rust_i18n::t!("parts.title", locale = l).to_string(),
            t_nav_configurations: rust_i18n::t!("catalog.configurations", locale = l).to_string(),
            t_nav_cables: rust_i18n::t!("cables.title", locale = l).to_string(),
            t_nav_software: rust_i18n::t!("software.title", locale = l).to_string(),
            t_nav_vlans: rust_i18n::t!("vlans.title", locale = l).to_string(),
            t_nav_merge: rust_i18n::t!("merge.title", locale = l).to_string(),
            t_nav_account: rust_i18n::t!("nav.account", locale = l).to_string(),
        }
    }

    /// 共有カタログの画面。`sub` は下位メニューのどの項目にいるか（空ならダッシュボード）。
    pub fn catalog(user: &entity::app_user::Model, csrf_token: String, sub: &'static str) -> Self {
        Self {
            sub,
            ..Self::new(user, csrf_token, "catalog")
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

// ---------------------------------------------------------------------------
// 語彙の表示（#126）
// ---------------------------------------------------------------------------

/// `<select>` の選択肢1つ。**`value` は保存する語彙のまま、`label` だけを訳す。**
pub struct Choice {
    pub value: &'static str,
    pub label: String,
}

/// 機器の種別（`device_category`）の表示名。
///
/// **訳すのは表示だけで、保存する値は語彙のまま。**略語（`VPN` / `PDU` /
/// `UPS` / `KVM`）は日本語画面でも訳さない。語彙に無い値は、そのまま出す。
pub fn 種別の表示(value: &str, locale: &str) -> String {
    let key = match value {
        "Server" => "device_categories.server",
        "Switch" => "device_categories.switch",
        "Router" => "device_categories.router",
        "Firewall" => "device_categories.firewall",
        "LoadBalancer" => "device_categories.load_balancer",
        "VPN" => "device_categories.vpn",
        "MediaConverter" => "device_categories.media_converter",
        "Storage" => "device_categories.storage",
        "PDU" => "device_categories.pdu",
        "UPS" => "device_categories.ups",
        "KVM" => "device_categories.kvm",
        "ConsoleServer" => "device_categories.console_server",
        "Other" => "device_categories.other",
        _ => return value.to_owned(),
    };
    rust_i18n::t!(key, locale = locale).to_string()
}

/// 機器の種別の選択肢。並びは語彙の表（`DEVICE_CATEGORIES`）のまま。
pub fn 種別の選択肢(locale: &str) -> Vec<Choice> {
    dioryga_catalog_format::DEVICE_CATEGORIES
        .iter()
        .map(|v| Choice {
            value: v,
            label: 種別の表示(v, locale),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **語彙のすべてに両言語の表示名があり、略語は訳さないこと**（#126）。
    ///
    /// 訳語が抜けると、キーの文字列（`device_categories.server`）が画面に出る。
    #[test]
    fn 種別の表示名が両言語にそろっている() {
        for v in dioryga_catalog_format::DEVICE_CATEGORIES {
            for locale in ["ja", "en"] {
                let label = 種別の表示(v, locale);
                assert!(!label.contains("device_categories"), "{locale}: {v}");
            }
            // 英語画面は語彙の値そのまま
            assert_eq!(種別の表示(v, "en"), *v);
        }
        assert_eq!(種別の表示("Server", "ja"), "サーバー");
        assert_eq!(種別の表示("ConsoleServer", "ja"), "コンソールサーバー");
        for 略語 in ["VPN", "PDU", "UPS", "KVM"] {
            assert_eq!(種別の表示(略語, "ja"), 略語);
        }
        // 語彙外（取込の検証より前に入った値など）はそのまま出す
        assert_eq!(種別の表示("でたらめ", "ja"), "でたらめ");
    }

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

    /// **リンクの既定色を消さないこと**（#127）。
    ///
    /// 消すと、文脈ごとの指定から漏れたリンクがブラウザ既定の青に戻り、
    /// ダークモードで読めなくなる。見た目の不具合は結合テストでは落ちない。
    #[test]
    fn リンクの既定色がライトとダークの両方にある() {
        let file = Assets::get("dioryga.css").expect("CSSが埋め込まれていません");
        let css = std::str::from_utf8(&file.data).unwrap();

        assert!(
            css.contains("a, a:visited { color: var(--link)"),
            "リンクの既定色の指定がありません"
        );
        let dark = css
            .split("@media (prefers-color-scheme: dark)")
            .nth(1)
            .expect("ダークモードの指定がありません");
        assert!(
            css.split("@media").next().unwrap().contains("--link:"),
            "ライトの --link がありません"
        );
        assert!(
            dark.split('}').next().unwrap().contains("--link:"),
            "ダークの --link がありません"
        );
    }

    #[test]
    fn 静的アセットが埋め込まれている() {
        assert!(
            Assets::get("dioryga.css").is_some(),
            "CSSがバイナリに埋め込まれていません"
        );
    }
}
