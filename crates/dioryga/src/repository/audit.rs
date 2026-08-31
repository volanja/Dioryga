//! 監査ログに載せる情報の定義。
//!
//! 設計書21.7およびCLAUDE.mdの不変条件により、**機微なカラムの平文を
//! `before_json` / `after_json` に残してはならない。**どのカラムを伏せるかは
//! テーブルごとにここで宣言する。

use serde::Serialize;
use serde_json::Value;

/// 監査ログに記録できるモデル。
///
/// エンティティごとに実装し、テーブル名・主キー・伏せるカラムを宣言する。
pub trait Audited: Serialize {
    /// `AUDIT_LOG.table_name` に記録する物理テーブル名。
    const TABLE: &'static str;

    /// 監査ログに平文で残してはならないカラム。
    const MASKED: &'static [&'static str] = &[];

    /// `AUDIT_LOG.record_id` に記録する主キー。
    fn audit_id(&self) -> i32;
}

/// マスキング後の値を示す文字列。値の有無だけは分かるようにしておく
/// （「元々空だった」と「伏せた」を区別できるようにするため）。
const MASKED_PLACEHOLDER: &str = "***";

/// モデルをJSONへ変換し、宣言されたカラムを伏せる。
pub fn to_masked_json<A: Audited>(model: &A) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(model)?;

    if let Value::Object(ref mut map) = value {
        for column in A::MASKED {
            if let Some(slot) = map.get_mut(*column) {
                *slot = Value::String(MASKED_PLACEHOLDER.to_owned());
            }
        }
    }

    serde_json::to_string(&value)
}

// ---------------------------------------------------------------------------
// エンティティごとの宣言
// ---------------------------------------------------------------------------

impl Audited for entity::app_user::Model {
    const TABLE: &'static str = "app_user";
    /// パスワードハッシュは監査ログに残さない。
    const MASKED: &'static [&'static str] = &["password_hash"];

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::project::Model {
    const TABLE: &'static str = "project";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::project_member::Model {
    const TABLE: &'static str = "project_member";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

// --- 共有カタログ（設計書6章、8章、9章、18章） ---
//
// **いずれも伏せる列を持たない。**カタログは型番・仕様といった公開情報であり、
// 監査ログでそのまま読めることに価値がある（誰がいつ何を変えたかを追うため）。
// 機微な値を持つ列を足すときは `MASKED` の宣言を忘れないこと。

impl Audited for entity::vendor::Model {
    const TABLE: &'static str = "vendor";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::chassis_model::Model {
    const TABLE: &'static str = "chassis_model";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::chassis_slot::Model {
    const TABLE: &'static str = "chassis_slot";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::part_catalog::Model {
    const TABLE: &'static str = "part_catalog";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::part_port_slot::Model {
    const TABLE: &'static str = "part_port_slot";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::configuration::Model {
    const TABLE: &'static str = "configuration";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::configuration_part::Model {
    const TABLE: &'static str = "configuration_part";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::cable_catalog::Model {
    const TABLE: &'static str = "cable_catalog";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::cable_end_slot::Model {
    const TABLE: &'static str = "cable_end_slot";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::software_catalog::Model {
    const TABLE: &'static str = "software_catalog";

    fn audit_id(&self) -> i32 {
        self.id
    }
}

impl Audited for entity::session::Model {
    const TABLE: &'static str = "session";
    /// セッショントークンのハッシュは、それ自体が認証に使える値のため残さない。
    const MASKED: &'static [&'static str] = &["token_hash"];

    fn audit_id(&self) -> i32 {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn 利用者() -> entity::app_user::Model {
        entity::app_user::Model {
            id: 1,
            name: "検証用".to_owned(),
            email: "masked@example.com".to_owned(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$abc$def".to_owned(),
            must_change_password: false,
            is_system_admin: false,
            locale: "ja".to_owned(),
            last_login_at: None,
            disabled_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn マスキング対象のカラムは平文で現れない() {
        let json = to_masked_json(&利用者()).unwrap();

        assert!(
            !json.contains("argon2id"),
            "パスワードハッシュが漏れています"
        );
        assert!(json.contains(MASKED_PLACEHOLDER));
        // 伏せる対象でないカラムはそのまま残る
        assert!(json.contains("masked@example.com"));
    }

    #[test]
    fn マスキング対象が無いモデルはそのまま出る() {
        let project = entity::project::Model {
            id: 7,
            uid: "11111111-2222-3333-4444-555555555555".to_owned(),
            code: None,
            name: "検証プロジェクト".to_owned(),
            description: String::new(),
            currency: "JPY".to_owned(),
            archived_at: None,
            closure_reason: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = to_masked_json(&project).unwrap();
        assert!(json.contains("検証プロジェクト"));
        assert!(!json.contains(MASKED_PLACEHOLDER));
    }
}
