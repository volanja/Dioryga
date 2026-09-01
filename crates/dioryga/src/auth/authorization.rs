//! 認可（設計書3章、20.10）。
//!
//! 認証（誰か）と認可（何をしてよいか）を分けて扱う。本モジュールは後者。

use entity::{app_user, project_member};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

/// プロジェクトロール。
pub const ADMINISTRATOR: &str = "Administrator";
pub const OPERATOR: &str = "Operator";
pub const APPROVER: &str = "Approver";
pub const VIEWER: &str = "Viewer";

#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("この操作を行う権限がありません")]
    Denied,

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),
}

/// System Adminがプロジェクト内データへ触れようとしていないか。
///
/// **一般的なRBACと異なり、System Adminはプロジェクト内データへのアクセス権を
/// 一切持たない**（設計書3章）。脆弱性等でSystem Admin権限が奪われた際の
/// 被害範囲を限定するための、意図的な逆転制約である。
///
/// **ロールの強弱の判定とは独立した関数として置いている。**両者を混ぜると、
/// ロール判定の変更が意図せずこの制約を壊しうるため。
pub fn deny_system_admin(user: &app_user::Model) -> Result<(), AuthzError> {
    if user.is_system_admin {
        return Err(AuthzError::Denied);
    }
    Ok(())
}

/// 指定したプロジェクトで、要求されるロールのいずれかを持つか。
///
/// **ユーザーは同一プロジェクトで複数のロールを兼務しうる**（設計書5章）。
/// そのため「いずれかのロールが要件を満たすか」で判定する。
pub async fn require_project_role<C: ConnectionTrait>(
    db: &C,
    user: &app_user::Model,
    project_id: i32,
    allowed: &[&str],
) -> Result<(), AuthzError> {
    // System Adminはロールに関わらず拒否する（3章）
    deny_system_admin(user)?;

    if user.disabled_at.is_some() {
        return Err(AuthzError::Denied);
    }

    let roles = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(user.id))
        .filter(project_member::Column::ProjectId.eq(project_id))
        .all(db)
        .await?;

    let 該当あり = roles.iter().any(|m| allowed.contains(&m.role.as_str()));

    if 該当あり {
        Ok(())
    } else {
        Err(AuthzError::Denied)
    }
}

/// 閲覧できるか。ロールを問わず、メンバーであればよい。
pub async fn require_project_member<C: ConnectionTrait>(
    db: &C,
    user: &app_user::Model,
    project_id: i32,
) -> Result<(), AuthzError> {
    require_project_role(
        db,
        user,
        project_id,
        &[ADMINISTRATOR, OPERATOR, APPROVER, VIEWER],
    )
    .await
}

/// プロジェクト内のデータを編集できるか。
///
/// **ApproverとViewerは編集できない。**Approverは承認する立場であり、自分で
/// 変更を入れられると承認の意味が薄れる（11章）。
pub async fn require_project_editor<C: ConnectionTrait>(
    db: &C,
    user: &app_user::Model,
    project_id: i32,
) -> Result<(), AuthzError> {
    require_project_role(db, user, project_id, &[ADMINISTRATOR, OPERATOR]).await
}

/// カタログマスタを編集できるか（設計書18.1）。
///
/// > いずれか1つ以上のプロジェクトでOperator以上のロールを持つUserであれば、
/// > カタログマスタの新規作成・編集ができる。
///
/// **System Adminは編集できない。**プロジェクトデータに触れない制約を維持するため。
pub async fn require_catalog_editor<C: ConnectionTrait>(
    db: &C,
    user: &app_user::Model,
) -> Result<(), AuthzError> {
    deny_system_admin(user)?;

    if user.disabled_at.is_some() {
        return Err(AuthzError::Denied);
    }

    let 該当あり = project_member::Entity::find()
        .filter(project_member::Column::UserId.eq(user.id))
        .filter(project_member::Column::Role.is_in([ADMINISTRATOR, OPERATOR]))
        .one(db)
        .await?
        .is_some();

    if 該当あり {
        Ok(())
    } else {
        Err(AuthzError::Denied)
    }
}
