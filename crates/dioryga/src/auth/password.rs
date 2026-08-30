//! パスワードのハッシュ化と検証（設計書20.2、20.3）。
//!
//! # なぜArgon2idか
//!
//! メモリハード関数であり、GPU/ASICによる並列総当たりに対して費用面での耐性を持つ。
//! RustCryptoの実装はpure Rustであり、Cツールチェーンに依存しないため、
//! Windows向けビルドを単一バイナリで完結させる方針（2章）とも合う。
//!
//! # PHC string format
//!
//! ソルトとパラメータがハッシュ文字列に内包されるため、`app_user` が持つ列は
//! `password_hash` の1本でよい。将来パラメータを引き上げた際も、
//! [`Verified::needs_rehash`] により利用者にパスワード再設定を強いずに移行できる。

use argon2::password_hash::phc::PasswordHash;
use argon2::{Algorithm, Argon2, Params, PasswordHasher as _, PasswordVerifier, Version};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use tokio::sync::Semaphore;

use crate::config::PasswordConfig;

/// 検証に成功した場合の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    /// 保存されているハッシュが現行のパラメータより弱いか。
    ///
    /// 真であれば、**平文が手元にあるこのタイミングで**再ハッシュして保存し直す
    /// （設計書20.2）。ログイン処理以外に平文を得る機会はない。
    pub needs_rehash: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("パスワードは{min}文字以上である必要があります")]
    TooShort { min: usize },

    #[error("パスワードは{max}文字以下である必要があります")]
    TooLong { max: usize },

    #[error("よく使われるパスワードは使用できません")]
    TooCommon,

    #[error("メールアドレスや名前を含むパスワードは使用できません")]
    ContainsIdentity,

    #[error("パスワードの処理に失敗しました: {0}")]
    Internal(String),
}

/// よく使われるパスワードの最小限のリスト。
///
/// 設計書20.3の通り、外部API（Have I Been Pwned等）への照会は行わない。
/// 完全なリストの同梱もバイナリサイズの観点で採らず、埋め込みリストで代替する。
///
/// **最小長（既定12文字）より短い項目を載せても意味がない。**長さの検証が先に
/// 走るため到達しないためである。ここには「十分に長いが弱い」ものだけを置く。
const COMMON_PASSWORDS: &[&str] = &[
    "password1234",
    "password12345",
    "passw0rd1234",
    "administrator",
    "123456789012",
    "1234567890123",
    "qwertyuiop123",
    "qwertyuiopasdf",
    "letmein12345",
    "welcome123456",
    "iloveyou1234",
    "changeme1234",
    "adminadmin12",
    "dioryga12345",
];

/// パスワードのハッシュ化を行う。
///
/// # 同時実行数の制限
///
/// Argon2idは1回あたり設定されたメモリ（既定19MiB）を占有する。制限しないと
/// ログインの集中がメモリ枯渇を招く——メモリハード関数を採用したことの裏返しの
/// リスクである（設計書24.2、21.8.6）。セマフォで同時実行数に上限を設ける。
///
/// また計算はCPUバウンドであるため、非同期ランタイムのワーカーを塞がないよう
/// ブロッキングスレッドで実行する。
pub struct PasswordService {
    config: PasswordConfig,
    /// 同時に走るArgon2idの数を制限する。
    permits: Semaphore,
    /// ユーザーが存在しない場合の検証に使うハッシュ（設計書20.6）。
    dummy_hash: String,
}

impl PasswordService {
    pub fn new(config: PasswordConfig) -> Result<Self, PasswordError> {
        let service = Self {
            permits: Semaphore::new(config.max_concurrent_hashes),
            dummy_hash: String::new(),
            config,
        };

        // 存在しないユーザーに対して検証を行うためのハッシュを1つ用意する。
        let dummy_hash = service.hash_blocking("dioryga-dummy-password")?;
        Ok(Self {
            dummy_hash,
            ..service
        })
    }

    fn argon2(&self) -> Result<Argon2<'static>, PasswordError> {
        let params = Params::new(
            self.config.memory_kib,
            self.config.iterations,
            self.config.parallelism,
            None,
        )
        .map_err(|e| PasswordError::Internal(e.to_string()))?;

        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    fn hash_blocking(&self, password: &str) -> Result<String, PasswordError> {
        // ソルトはユーザーごとにCSPRNGで生成される（hash_password が内部で行う）。
        let hash: PasswordHash = self
            .argon2()?
            .hash_password(password.as_bytes())
            .map_err(|e| PasswordError::Internal(e.to_string()))?;
        Ok(hash.to_string())
    }

    fn verify_blocking(
        &self,
        password: &str,
        phc: &str,
    ) -> Result<Option<Verified>, PasswordError> {
        let parsed = PasswordHash::new(phc).map_err(|e| PasswordError::Internal(e.to_string()))?;

        if self
            .argon2()?
            .verify_password(password.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }

        Ok(Some(Verified {
            needs_rehash: self.is_outdated(&parsed),
        }))
    }

    /// 保存されているハッシュのパラメータが現行の設定より弱いか。
    fn is_outdated(&self, parsed: &PasswordHash) -> bool {
        let Ok(params) = Params::try_from(parsed) else {
            // 解釈できない形式であれば作り直す
            return true;
        };

        params.m_cost() < self.config.memory_kib
            || params.t_cost() < self.config.iterations
            || params.p_cost() < self.config.parallelism
    }

    pub async fn hash(&self, password: &str) -> Result<String, PasswordError> {
        let _permit = self.acquire().await?;
        let password = password.to_owned();
        self.run_blocking(move |svc| svc.hash_blocking(&password))
            .await
    }

    /// パスワードを検証する。一致しなければ `None`。
    pub async fn verify(
        &self,
        password: &str,
        phc: &str,
    ) -> Result<Option<Verified>, PasswordError> {
        let _permit = self.acquire().await?;
        let password = password.to_owned();
        let phc = phc.to_owned();
        self.run_blocking(move |svc| svc.verify_blocking(&password, &phc))
            .await
    }

    /// 存在しないユーザーに対しても検証を行い、応答時間の差を消す（設計書20.6）。
    ///
    /// これを行わないと「存在しないユーザーは即座に返る／存在するユーザーは
    /// 数百ミリ秒かかる」という差から、アカウントの存在有無を判別できてしまう。
    pub async fn verify_dummy(&self, password: &str) -> Result<(), PasswordError> {
        let _ = self.verify(password, &self.dummy_hash).await?;
        Ok(())
    }

    async fn acquire(&self) -> Result<tokio::sync::SemaphorePermit<'_>, PasswordError> {
        self.permits
            .acquire()
            .await
            .map_err(|e| PasswordError::Internal(e.to_string()))
    }

    /// CPUバウンドな処理をブロッキングスレッドで実行する。
    async fn run_blocking<T, F>(&self, f: F) -> Result<T, PasswordError>
    where
        F: FnOnce(&Self) -> Result<T, PasswordError> + Send + 'static,
        T: Send + 'static,
    {
        // 設定はコピーが安く、Argon2の実行に必要なのはそれだけ。
        let svc = Self {
            config: self.config.clone(),
            permits: Semaphore::new(1),
            dummy_hash: String::new(),
        };

        tokio::task::spawn_blocking(move || f(&svc))
            .await
            .map_err(|e| PasswordError::Internal(e.to_string()))?
    }

    /// ポリシーを満たすか検証する（設計書20.3）。
    ///
    /// **文字種の強制と定期変更の強制は課さない。**複雑性要件は `Password1!` の
    /// ような予測可能な変形を誘発するだけで実効強度が上がらないため。
    pub fn check_policy(
        &self,
        password: &str,
        email: &str,
        name: &str,
    ) -> Result<(), PasswordError> {
        let length = password.chars().count();
        if length < self.config.min_length {
            return Err(PasswordError::TooShort {
                min: self.config.min_length,
            });
        }
        if length > self.config.max_length {
            return Err(PasswordError::TooLong {
                max: self.config.max_length,
            });
        }

        let lower = password.to_lowercase();
        if COMMON_PASSWORDS.contains(&lower.as_str()) {
            return Err(PasswordError::TooCommon);
        }

        let local_part = email.split('@').next().unwrap_or(email);
        if !local_part.is_empty() && lower.contains(&local_part.to_lowercase()) {
            return Err(PasswordError::ContainsIdentity);
        }
        if !name.is_empty() && lower.contains(&name.to_lowercase()) {
            return Err(PasswordError::ContainsIdentity);
        }

        Ok(())
    }
}

/// 一時パスワードを生成する（設計書20.7）。
///
/// **平文はここで返す一度きりしか存在しない。**DBにはArgon2idハッシュだけを
/// 保存し、発行者は画面またはコンソールで受け取って本人へ別経路で伝える。
///
/// 画面（System Adminのユーザー管理）とCLIの復旧経路の両方から使うため、
/// どちらにも属さないここに置いている。
pub fn generate_temporary() -> Result<String, PasswordError> {
    let mut bytes = [0u8; 18];
    getrandom::fill(&mut bytes).map_err(|e| PasswordError::Internal(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> PasswordService {
        // テストでは計算量を落とす。本番の既定値は設計書20.2（OWASPの最小構成）。
        PasswordService::new(PasswordConfig {
            memory_kib: 8,
            iterations: 1,
            parallelism: 1,
            ..PasswordConfig::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn 正しいパスワードは検証に成功する() {
        let svc = service();
        let phc = svc.hash("正しいパスワードです").await.unwrap();

        let 結果 = svc.verify("正しいパスワードです", &phc).await.unwrap();
        assert!(結果.is_some());
        assert!(!結果.unwrap().needs_rehash);
    }

    #[tokio::test]
    async fn 誤ったパスワードは検証に失敗する() {
        let svc = service();
        let phc = svc.hash("正しいパスワードです").await.unwrap();

        assert!(svc
            .verify("誤ったパスワード", &phc)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn phc形式で保存される() {
        let phc = service().hash("検証用のパスワード").await.unwrap();
        assert!(phc.starts_with("$argon2id$v=19$"), "PHC形式ではない: {phc}");
    }

    #[tokio::test]
    async fn 同じパスワードでも毎回異なるハッシュになる() {
        let svc = service();
        let a = svc.hash("同一のパスワード").await.unwrap();
        let b = svc.hash("同一のパスワード").await.unwrap();
        assert_ne!(a, b, "ソルトが効いていません");
    }

    #[tokio::test]
    async fn パラメータが弱いハッシュは再ハッシュが必要と判定される() {
        let 弱い設定 = service();
        let phc = 弱い設定.hash("検証用のパスワード").await.unwrap();

        let 強い設定 = PasswordService::new(PasswordConfig {
            memory_kib: 32,
            iterations: 2,
            parallelism: 1,
            ..PasswordConfig::default()
        })
        .unwrap();

        let 結果 = 強い設定.verify("検証用のパスワード", &phc).await.unwrap();
        assert!(結果.is_some(), "検証自体は成功する必要がある");
        assert!(結果.unwrap().needs_rehash);
    }

    #[tokio::test]
    async fn 存在しない利用者でもダミー検証が走る() {
        assert!(service().verify_dummy("なんらかの入力").await.is_ok());
    }

    #[test]
    fn 短すぎるパスワードは拒否される() {
        let svc = service();
        let 結果 = svc.check_policy("short", "user@example.com", "利用者");
        assert!(matches!(結果, Err(PasswordError::TooShort { .. })));
    }

    #[test]
    fn 文字種を強制しない() {
        // 小文字のみでも、十分な長さがあれば通す（設計書20.3）
        let svc = service();
        assert!(svc
            .check_policy("correcthorsebatterystaple", "user@example.com", "利用者")
            .is_ok());
    }

    #[test]
    fn よくあるパスワードは拒否される() {
        let svc = service();
        assert!(matches!(
            svc.check_policy("password1234", "user@example.com", "利用者"),
            Err(PasswordError::TooCommon)
        ));
    }

    #[test]
    fn ブロックリストの項目はすべて最小長を満たす() {
        // 最小長より短い項目は長さの検証で弾かれるため到達しない。
        // リストに載せても効かないので、混入していないことを確かめる。
        let svc = service();
        for 項目 in COMMON_PASSWORDS {
            assert!(
                項目.chars().count() >= svc.config.min_length,
                "到達しない項目がブロックリストにあります: {項目}"
            );
        }
    }

    #[test]
    fn メールアドレスや名前を含むと拒否される() {
        let svc = service();
        assert!(matches!(
            svc.check_policy("tanaka-no-password", "tanaka@example.com", "田中"),
            Err(PasswordError::ContainsIdentity)
        ));
        assert!(matches!(
            svc.check_policy("dioryga-tanaka-2026", "user@example.com", "tanaka"),
            Err(PasswordError::ContainsIdentity)
        ));
    }

    #[test]
    fn 長すぎるパスワードは拒否される() {
        let svc = service();
        let 長い = "あ".repeat(300);
        assert!(matches!(
            svc.check_policy(&長い, "user@example.com", "利用者"),
            Err(PasswordError::TooLong { .. })
        ));
    }

    #[test]
    fn 一時パスワードは毎回異なる() {
        assert_ne!(generate_temporary().unwrap(), generate_temporary().unwrap());
    }

    /// 生成した一時パスワードがポリシーを満たすこと。
    ///
    /// 満たさないと、発行はできるのに本人がログイン後の変更画面まで辿り着けない、
    /// という気付きにくい不整合になる。
    #[test]
    fn 一時パスワードはポリシーを満たす() {
        let svc = service();
        let temporary = generate_temporary().unwrap();
        assert!(
            svc.check_policy(&temporary, "user@example.com", "利用者")
                .is_ok(),
            "生成した一時パスワードがポリシーに反しています: {temporary}"
        );
    }
}
