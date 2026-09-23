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

/// 表示モード（設計書16.4、#120）。**既定はOSに従う。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    /// `USER.theme` の値。読めない値は既定（OSに従う）にする。
    pub fn parse(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// `<html>` の `data-theme`。**OSに従うときは属性を置かない**
    /// ——CSSの `prefers-color-scheme` にそのまま任せる。
    pub fn attribute(self) -> &'static str {
        match self {
            Self::System => "",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// ログイン後の画面が共通で持つ枠（`app_layout.html`）。
///
/// **各ページの構造体にこれを1つ持たせる。**ヘッダーとナビに必要な値は
/// どの画面でも同じであり、画面を足すたびに同じフィールドを並べ直したくない。
///
/// # メニューの形（#122）
///
/// - **上部**：領域（プロジェクト／倉庫／共有カタログ／個人設定）。頻繁には切り替えない
/// - **左**：**今いる領域の中の画面。**頻繁に行き来するのはこちら
///
/// どちらも**今いる場所に印を付ける。**詳細画面では親の一覧に付ける（#118）。
/// 印が2つ付くとどちらにいるのか読めないため、左メニューの印は常に1つにする。
pub struct Chrome {
    pub locale: &'static str,
    /// `<html data-theme>` に出す値。OSに従うときは空（#120）。
    pub theme: &'static str,
    pub app_name: String,
    pub user_name: String,
    /// フォームのhidden fieldへ埋め込むCSRFトークン（設計書20.5）。
    pub csrf_token: String,
    pub t_logout: String,
    /// パンくずで使う。
    pub t_nav_projects: String,
    /// 上部のメニュー（領域）。
    pub top: Vec<NavItem>,
    /// 左のメニュー（今いる領域の中の画面）。
    pub side: Vec<NavItem>,
    /// 画面下のステータスバーに出す項目（#162）。**画面ごとに差し替える。**
    /// 出す内容が無い画面では空のままでよい——版は常に右端へ出る。
    pub status: Vec<String>,
    /// 右端に出す版。どの画面でも同じ。
    pub t_version: String,
}

/// メニューの1項目。
pub struct NavItem {
    /// 見出し（プロジェクト名・倉庫名）はリンクにしない。
    pub href: Option<String>,
    pub label: String,
    pub current: bool,
    /// 見出しの下に字下げして並べる項目。
    pub sub: bool,
}

impl NavItem {
    /// `class` 属性の値。字下げ（`sub`）と印（`current`）を並べる。
    pub fn class(&self) -> &'static str {
        match (self.sub, self.current) {
            (true, true) => "sub current",
            (true, false) => "sub",
            (false, true) => "current",
            (false, false) => "",
        }
    }

    fn link(href: impl Into<String>, label: String, current: bool) -> Self {
        Self {
            href: Some(href.into()),
            label,
            current,
            sub: false,
        }
    }

    fn sub(href: impl Into<String>, label: String, current: bool) -> Self {
        Self {
            sub: true,
            ..Self::link(href, label, current)
        }
    }

    fn heading(label: String) -> Self {
        Self {
            href: None,
            label,
            current: false,
            sub: false,
        }
    }
}

/// プロジェクトの中の画面（左メニュー）。`(sub, パス, ラベルのキー)`。
///
/// **並びは使う頻度の順**：機器・什器・ネットワークを上に、管理（メンバー・取込）を下に。
const プロジェクトの画面: &[(&str, &str, &str)] = &[
    ("dashboard", "", "nav.dashboard"),
    ("devices", "/devices", "nav.devices"),
    ("containers", "/containers", "nav.containers"),
    ("ip_addresses", "/network/ip-addresses", "nav.ip_addresses"),
    ("components", "/software/components", "nav.components"),
    ("work_orders", "/work-orders", "nav.work_orders"),
    ("milestones", "/milestones", "nav.milestones"),
    ("costs", "/costs", "nav.costs"),
    ("power", "/power", "nav.power"),
    ("members", "/members", "nav.members"),
];

/// 共有カタログの中の画面（#118）。`(sub, パス, ラベルのキー)`。
const カタログの画面: &[(&str, &str, &str)] = &[
    ("", "/catalog", "nav.dashboard"),
    ("vendors", "/catalog/vendors", "catalog.vendors"),
    (
        "chassis_models",
        "/catalog/chassis-models",
        "catalog.chassis_models",
    ),
    ("parts", "/catalog/parts", "parts.title"),
    (
        "configurations",
        "/catalog/configurations",
        "catalog.configurations",
    ),
    ("cables", "/catalog/cables", "cables.title"),
    ("software", "/catalog/software", "software.title"),
    ("vlans", "/catalog/vlans", "vlans.title"),
    ("merge", "/catalog/merge", "merge.title"),
];

fn t(key: &str, l: &str) -> String {
    rust_i18n::t!(key, locale = l).to_string()
}

impl Chrome {
    /// 領域の一覧画面。`nav` は `projects` / `warehouses` / `account` /
    /// `users` / `admin_projects`。左メニューはその一覧だけを持つ。
    pub fn new(user: &entity::app_user::Model, csrf_token: String, nav: &'static str) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        let side = match nav {
            "projects" => vec![NavItem::link("/projects", t("nav.project_list", l), true)],
            "warehouses" => vec![NavItem::link(
                "/warehouses",
                t("nav.warehouse_list", l),
                true,
            )],
            // 個人設定は [`Chrome::account`] を使う。ここへは来ない
            "account" => Vec::new(),
            "users" => vec![NavItem::link("/admin/users", t("nav.user_list", l), true)],
            "admin_projects" => {
                vec![NavItem::link(
                    "/admin/projects",
                    t("nav.project_list", l),
                    true,
                )]
            }
            _ => Vec::new(),
        };
        Self::組む(user, csrf_token, nav, side)
    }

    /// 個人設定の画面（#120）。`sub` は `display` / `password`。
    ///
    /// **性格の違う操作を分ける。**表示は選んで保存するだけだが、パスワードは
    /// 現行の入力が要り、他の端末のセッションを切る（20.7）。
    pub fn account(user: &entity::app_user::Model, csrf_token: String, sub: &'static str) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        let side = vec![
            NavItem::link("/account/display", t("nav.display", l), sub == "display"),
            NavItem::link("/account/password", t("nav.password", l), sub == "password"),
        ];
        Self::組む(user, csrf_token, "account", side)
    }

    /// 共有カタログの画面。`sub` は左メニューのどの項目にいるか（空ならダッシュボード）。
    pub fn catalog(user: &entity::app_user::Model, csrf_token: String, sub: &'static str) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        let side = カタログの画面
            .iter()
            .map(|(key, href, label)| NavItem::link(*href, t(label, l), *key == sub))
            .collect();
        Self::組む(user, csrf_token, "catalog", side)
    }

    /// プロジェクトの中の画面（#122、#123）。
    ///
    /// `sub` は `dashboard` / `devices` / `containers` / `ip_addresses` /
    /// `components` / `work_orders` / `milestones` / `costs` / `power` /
    /// `members` / `import`。**取込は編集権のある利用者にだけ出す**——
    /// 押しても入れない項目を並べない。
    pub async fn project<C: sea_orm::ConnectionTrait>(
        db: &C,
        user: &entity::app_user::Model,
        csrf_token: String,
        project: &entity::project::Model,
        sub: &'static str,
    ) -> Self {
        let 編集できる = crate::auth::authorization::require_project_editor(db, user, project.id)
            .await
            .is_ok();
        Self::project_known(user, csrf_token, project, sub, 編集できる)
    }

    /// [`Chrome::project`] の、編集権を呼び出し側が既に知っている場合。
    ///
    /// 取込の結果画面のように、**編集権を確かめてから入る画面**で使う。
    pub fn project_known(
        user: &entity::app_user::Model,
        csrf_token: String,
        project: &entity::project::Model,
        sub: &'static str,
        編集できる: bool,
    ) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        let base = format!("/projects/{}", project.id);
        let mut side = vec![
            NavItem::link("/projects", t("nav.project_list", l), false),
            NavItem::heading(project.name.clone()),
        ];
        side.extend(プロジェクトの画面.iter().map(|(key, path, label)| {
            NavItem::sub(format!("{base}{path}"), t(label, l), *key == sub)
        }));
        if 編集できる {
            side.push(NavItem::sub(
                format!("{base}/import"),
                t("nav.import", l),
                sub == "import",
            ));
        }
        Self::組む(user, csrf_token, "projects", side)
    }

    /// 倉庫の中の画面（#122）。`sub` は `devices` / `parts`。
    pub fn warehouse(
        user: &entity::app_user::Model,
        csrf_token: String,
        warehouse_id: i32,
        warehouse_name: &str,
        sub: &'static str,
    ) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        let base = format!("/warehouses/{warehouse_id}");
        let side = vec![
            NavItem::link("/warehouses", t("nav.warehouse_list", l), false),
            NavItem::heading(warehouse_name.to_owned()),
            NavItem::sub(
                format!("{base}/devices"),
                t("nav.stored_devices", l),
                sub == "devices",
            ),
            NavItem::sub(
                format!("{base}/parts"),
                t("nav.stored_parts", l),
                sub == "parts",
            ),
        ];
        Self::組む(user, csrf_token, "warehouses", side)
    }

    fn 組む(
        user: &entity::app_user::Model,
        csrf_token: String,
        nav: &str,
        side: Vec<NavItem>,
    ) -> Self {
        let l = Locale::parse(&user.locale).as_str();
        // **System Adminはプロジェクトの中・倉庫・カタログに入れない**（3章）。
        // 出しても入れない項目は並べない
        let top = if user.is_system_admin {
            vec![
                NavItem::link("/admin/users", t("nav.users", l), nav == "users"),
                NavItem::link(
                    "/admin/projects",
                    t("nav.admin_projects", l),
                    nav == "admin_projects",
                ),
                // 個人設定の先頭の画面へ入る（#120）
                NavItem::link("/account/display", t("nav.account", l), nav == "account"),
            ]
        } else {
            vec![
                NavItem::link("/projects", t("nav.projects", l), nav == "projects"),
                NavItem::link("/warehouses", t("warehouses.title", l), nav == "warehouses"),
                NavItem::link("/catalog", t("catalog.nav", l), nav == "catalog"),
                // 個人設定の先頭の画面へ入る（#120）
                NavItem::link("/account/display", t("nav.account", l), nav == "account"),
            ]
        };
        Self {
            locale: l,
            theme: Theme::parse(&user.theme).attribute(),
            status: Vec::new(),
            t_version: format!("Dioryga {}", env!("CARGO_PKG_VERSION")),
            app_name: t("app.name", l),
            user_name: user.name.clone(),
            csrf_token,
            t_logout: t("common.logout", l),
            t_nav_projects: t("nav.projects", l),
            top,
            side,
        }
    }

    /// ステータスバーの項目を差し替える（#162）。
    ///
    /// 一覧なら表示件数、取込なら対象のファイルなど、**その画面で最後に見る値**を置く。
    pub fn with_status(mut self, items: Vec<String>) -> Self {
        self.status = items;
        self
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

/// 機器・部品の状態（8.6）の表示名。**訳すのは表示だけで、保存する値は語彙のまま。**
///
/// 値は表示のほかにCSSのクラス名にも使う（`.led.running` 等）ので、
/// **クラスには生の値を、文字にはこの表示名を**当てる。
pub fn 状態の表示(value: &str, locale: &str) -> String {
    let key = match value {
        "running" => "device_statuses.running",
        "planned" => "device_statuses.planned",
        "provisioning" => "device_statuses.provisioning",
        "repairing" => "device_statuses.repairing",
        "failed" => "device_statuses.failed",
        _ => return value.to_owned(),
    };
    rust_i18n::t!(key, locale = locale).to_string()
}

/// 機器の状態の選択肢。並びは語彙の表のまま。
pub fn 状態の選択肢(statuses: &[&'static str], locale: &str) -> Vec<Choice> {
    statuses
        .iter()
        .map(|v| Choice {
            value: v,
            label: 状態の表示(v, locale),
        })
        .collect()
}

/// 人が連絡に使うチケット番号（設計書11.4-11）。
///
/// **`id` から作る。列としては持たない。**`WORK_ORDER.id` はプロジェクトを
/// 横断して一意であり、「W-1042の件」で通じる。保存すると二重管理になる
/// （不変条件2）。取込の突合に使うのは `uid` / `external_id` のほう（23.5）。
pub fn チケット番号(id: i32) -> String {
    format!("W-{id}")
}

/// 変更管理チケットの状態（11章）の表示名。
pub fn チケットの状態の表示(value: &str, locale: &str) -> String {
    let key = match value {
        "planned" => "work_order_statuses.planned",
        "approved" => "work_order_statuses.approved",
        "in_progress" => "work_order_statuses.in_progress",
        "completed" => "work_order_statuses.completed",
        "cancelled" => "work_order_statuses.cancelled",
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

    /// **状態の表示名が両言語にそろっていること**（#172）。
    ///
    /// 訳語が抜けると、キーの文字列がそのまま画面に出る。
    #[test]
    fn 状態の表示名が両言語にそろっている() {
        for v in ["running", "planned", "provisioning", "repairing", "failed"] {
            for locale in ["ja", "en"] {
                assert!(
                    !状態の表示(v, locale).contains("device_statuses"),
                    "{locale}: {v}"
                );
            }
        }
        for v in [
            "planned",
            "approved",
            "in_progress",
            "completed",
            "cancelled",
        ] {
            for locale in ["ja", "en"] {
                assert!(
                    !チケットの状態の表示(v, locale).contains("work_order_statuses"),
                    "{locale}: {v}"
                );
            }
        }
        assert_eq!(状態の表示("running", "ja"), "稼働中");
        assert_eq!(状態の表示("failed", "ja"), "故障");
        assert_eq!(チケットの状態の表示("in_progress", "ja"), "実行中");
        assert_eq!(チケットの状態の表示("cancelled", "ja"), "中止");
        // 語彙外はそのまま出す（取込の検証より前に入った値など）
        assert_eq!(状態の表示("でたらめ", "ja"), "でたらめ");
    }

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
