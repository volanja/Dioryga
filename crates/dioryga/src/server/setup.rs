//! 初回セットアップの画面（設計書20.8）。
//!
//! 画面そのもの（Askamaテンプレート）はP3-4で作る。ここでは動作と遷移を用意する。

use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::setup;
use crate::error::{AppError, AppResult};
use crate::server::AppState;

#[derive(Debug, Deserialize)]
pub struct SetupForm {
    pub token: String,
    pub name: String,
    pub email: String,
    pub password: String,
}

/// セットアップ画面を表示する。
///
/// セットアップが完了していれば404を返す。**以降この経路が開くことはない**
/// （利用者が0件に戻ることがないため）。
pub async fn show(State(state): State<AppState>) -> AppResult<Response> {
    if !state.setup.is_pending().await {
        return Err(AppError::NotFound);
    }

    Ok(Html(
        "<!doctype html><meta charset=\"utf-8\"><title>初回セットアップ</title>\
         <p>初回セットアップ。画面はP3-4で実装する。</p>",
    )
    .into_response())
}

/// 最初のSystem Adminを作成する。
pub async fn submit(
    State(state): State<AppState>,
    Form(form): Form<SetupForm>,
) -> AppResult<Response> {
    setup::create_first_admin(
        &state.db,
        &state.setup,
        &state.passwords,
        &form.token,
        &form.name,
        &form.email,
        &form.password,
    )
    .await
    .map_err(|e| match e {
        setup::SetupError::AlreadyCompleted => AppError::NotFound,
        setup::SetupError::InvalidToken => AppError::Forbidden,
        setup::SetupError::Password(e) => AppError::Validation(e.to_string()),
        other => AppError::Internal(anyhow::anyhow!(other)),
    })?;

    Ok(Redirect::to("/login").into_response())
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

    // セットアップ自身とヘルスチェックは通す。
    // ヘルスチェックを通すのは、CIのスモークテストが起動直後に叩くため（2章）。
    let 通す = path == "/setup" || path == "/health";

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
        let json = r#"{"token":"t","name":"n","email":"e","password":"p"}"#;
        let form: SetupForm = serde_json::from_str(json).unwrap();
        assert_eq!(form.token, "t");
        assert_eq!(form.email, "e");
    }
}
