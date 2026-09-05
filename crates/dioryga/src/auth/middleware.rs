//! 認証・認可のミドルウェア（設計書20.10）。
//!
//! 2層に分ける。
//!
//! ```text
//! ① 認証：Cookie → セッション → User を解決する
//! ② 認可：User + 対象 → 許可 / 拒否
//! ```
//!
//! **System Adminガードは②の中でも独立した層として置く。**ロールの強弱の判定と
//! 混ぜると、ロール判定の変更が意図せず「System Adminはプロジェクトデータに
//! 触れない」という制約（3章）を壊しうるため。

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use chrono::Utc;
use entity::app_user;
use sea_orm::EntityTrait;

use crate::auth::{authorization, cookie, csrf, session};
use crate::server::AppState;

/// 認証済みの利用者。ハンドラからは拡張として取り出す。
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub user: app_user::Model,
    pub session_id: i32,
    /// このセッションに対応するCSRFトークン。フォームへ埋め込む。
    pub csrf_token: String,
}

/// 認証を要求しない経路。
///
/// **許可リスト方式にしている**（設計書20.10）。新しい画面を追加したときに
/// 保護し忘れる事故を防ぐため、ここに書いたもの以外はすべて認証必須になる。
fn is_public(path: &str) -> bool {
    // 静的アセットは認証の対象外。ログイン画面自体がCSSを読むため、
    // 保護すると未認証の画面が素のHTMLになってしまう。
    matches!(path, "/login" | "/setup" | "/health") || path.starts_with("/assets/")
}

/// パスワード変更を強制されている間でも通す経路。
///
/// 静的アセットを含めるのを忘れやすい。**除くとパスワード変更画面が
/// 素のHTMLになる**——画面は出るので気付きにくく、`is_public` と同じ穴である。
fn is_allowed_while_password_change_required(path: &str) -> bool {
    matches!(path, "/account/password" | "/logout" | "/health") || path.starts_with("/assets/")
}

/// ① 認証。
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();

    let raw_token = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(cookie::extract)
        .map(str::to_owned);

    let now = Utc::now();
    let current = match &raw_token {
        Some(token) => resolve(&state, token, now).await,
        None => None,
    };

    match current {
        Some(current) => {
            // パスワードの変更を終えるまで他の画面へ進ませない（設計書20.6）
            if current.user.must_change_password
                && !is_allowed_while_password_change_required(&path)
            {
                return Redirect::to("/account/password").into_response();
            }

            request.extensions_mut().insert(current);
            next.run(request).await
        }
        None if is_public(&path) => next.run(request).await,
        None => Redirect::to("/login").into_response(),
    }
}

/// セッションを検証し、利用者を解決する。
async fn resolve(
    state: &AppState,
    raw_token: &str,
    now: chrono::DateTime<Utc>,
) -> Option<CurrentUser> {
    let session = session::validate(&state.db, raw_token, &state.config.session, now)
        .await
        .ok()
        .flatten()?;

    let user = app_user::Entity::find_by_id(session.user_id)
        .one(&state.db)
        .await
        .ok()
        .flatten()?;

    // 無効化された利用者のセッションは通さない（設計書20.6）
    if user.disabled_at.is_some() {
        return None;
    }

    // アイドルタイムアウトの起点を更新する。
    // セッションは監査ログの対象外（設計書24.4）。
    let session_id = session.id;
    let _ = session::touch(&state.db, session, now).await;

    Some(CurrentUser {
        csrf_token: csrf::derive(raw_token),
        user,
        session_id,
    })
}

/// ② 認可：System Adminガード。
///
/// `is_system_admin=true` の利用者による `/projects/**` へのアクセスを、
/// **ロールの強弱に無関係に拒否する**（設計書3章）。
pub async fn system_admin_guard(request: Request, next: Next) -> Response {
    let path = request.uri().path();

    if path == "/projects" || path.starts_with("/projects/") {
        if let Some(current) = request.extensions().get::<CurrentUser>() {
            if authorization::deny_system_admin(&current.user).is_err() {
                return (
                    StatusCode::FORBIDDEN,
                    "System Adminはプロジェクト内のデータにアクセスできません",
                )
                    .into_response();
            }
        }
    }

    next.run(request).await
}

/// ② 認可：System Admin領域のガード。
///
/// [`system_admin_guard`] と対になる。あちらは「System Adminを入れない」、
/// こちらは「System Admin以外を入れない」。**両者を一つの関数にまとめない**のは、
/// 片方の条件を触ったときにもう片方を巻き添えにしないため。
pub async fn system_admin_only(request: Request, next: Next) -> Response {
    let path = request.uri().path();

    if path == "/admin" || path.starts_with("/admin/") {
        let 許可 = request
            .extensions()
            .get::<CurrentUser>()
            .is_some_and(|current| current.user.is_system_admin);

        if !許可 {
            return (
                StatusCode::FORBIDDEN,
                "System Adminのみが利用できる画面です",
            )
                .into_response();
        }
    }

    next.run(request).await
}

/// CSRFトークンの検証。
///
/// `SameSite=Lax` だけに頼らず、**状態変更を伴うリクエストにはトークンを要求する**
/// （設計書20.5）。
pub async fn verify_csrf(request: Request, next: Next) -> Response {
    let 状態変更 = matches!(
        request.method(),
        &axum::http::Method::POST
            | &axum::http::Method::PUT
            | &axum::http::Method::PATCH
            | &axum::http::Method::DELETE
    );

    if !状態変更 {
        return next.run(request).await;
    }

    // 未認証の経路（ログイン・セットアップ）は、そもそもセッションが無いため
    // CSRFトークンを持ちようがない。SameSite=Lax と、これらが認証情報を
    // 要求すること自体で守る。
    let Some(current) = request.extensions().get::<CurrentUser>().cloned() else {
        return next.run(request).await;
    };

    let provided = request
        .headers()
        .get(csrf::HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    if csrf::verify(&current.csrf_token, &provided) {
        return next.run(request).await;
    }

    // ヘッダーに無ければフォーム本文のhidden fieldを見る（設計書20.5）。
    //
    // **ヘッダーだけでは素のHTMLフォームを守れない。**`<form method="post">` は
    // ヘッダーを付けられないため、JavaScriptを前提にしない画面がすべて403になる。
    if is_form_urlencoded(&request) {
        return verify_from_form_body(request, next, &current.csrf_token, MAX_FORM_BODY).await;
    }

    // **ファイル添付のフォームも同じ扱いにする。**`enctype="multipart/form-data"`
    // でもヘッダーは付けられないため、ここを見ないと取込系の画面がすべて403に
    // なる——不変条件9（すべての画面は素のHTMLフォームだけで動く）が破れる。
    if is_multipart(&request) {
        return verify_from_form_body(request, next, &current.csrf_token, MAX_MULTIPART_BODY).await;
    }

    (StatusCode::FORBIDDEN, "CSRFトークンが不正です").into_response()
}

/// 本文から読み取るCSRFトークンの上限。
///
/// フォームの本文はいずれも小さい。ここを無制限にすると、本文を読み切るまで
/// メモリを確保し続けることになる。
const MAX_FORM_BODY: usize = 64 * 1024;

/// ファイル添付のフォームの上限。
///
/// 取込・SBOMの受け入れ上限（8MiB）に、境界と他のフィールドのぶんを足した値。
/// **ハンドラ側でも上限を確かめる**が、ここを通らないと本文を読み切れない。
const MAX_MULTIPART_BODY: usize = 9 * 1024 * 1024;

fn is_multipart(request: &Request) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"))
}

fn is_form_urlencoded(request: &Request) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"))
}

/// フォーム本文の `csrf_token` を検証し、**読み取った本文を差し戻す。**
///
/// 本文はストリームであり一度しか読めない。読んだまま次へ渡すとハンドラ側の
/// `Form` 抽出が空の本文を受け取ってしまうため、バイト列から組み立て直す。
async fn verify_from_form_body(
    request: Request,
    next: Next,
    expected: &str,
    limit: usize,
) -> Response {
    let multipart = is_multipart(&request);
    let (parts, body) = request.into_parts();

    let Ok(bytes) = axum::body::to_bytes(body, limit).await else {
        return (StatusCode::BAD_REQUEST, "リクエスト本文を読み取れません").into_response();
    };

    let provided = if multipart {
        multipart_field(&bytes, csrf::FIELD_NAME).unwrap_or_default()
    } else {
        form_urlencoded::parse(&bytes)
            .find(|(key, _)| key == csrf::FIELD_NAME)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };

    if !csrf::verify(expected, &provided) {
        return (StatusCode::FORBIDDEN, "CSRFトークンが不正です").into_response();
    }

    next.run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await
}

/// multipartの本文から、名前の付いた1つの値を取り出す。
///
/// **完全なパーサではない。**CSRFトークンだけを取れればよく、ここで
/// ファイル本体を読み解く必要はない。ハンドラ側が `Multipart` で改めて読む。
///
/// `Content-Disposition: form-data; name="csrf_token"` に続く空行の後から、
/// 次の境界の直前までを値とみなす。
fn multipart_field(bytes: &[u8], name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    let haystack = String::from_utf8_lossy(bytes);

    let start = haystack.find(&needle)?;
    let rest = &haystack[start..];
    // ヘッダーと本文は空行で区切られる
    let body_start = rest.find("\r\n\r\n")? + 4;
    let body = &rest[body_start..];
    let end = body.find("\r\n--")?;
    Some(body[..end].to_owned())
}
