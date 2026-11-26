//! Logins that race a second factor turned on meanwhile: the handler decides which factor to ask
//! from credentials read before a slow hash, so the session write must check that it still holds.

use cadocs_store::{
    AccountStore, CodeAttempt, CodePurpose, FirstUser, NewEmailCode, NewSession, SqliteStore,
    UpdateBasis,
};
use tempfile::TempDir;

const ADA: &str = "ada@example.org";
const HASH: &str = "hash";

async fn with_ada() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap();
    let first = FirstUser {
        id: "ada".to_owned(),
        email: ADA.to_owned(),
        name: "Ada".to_owned(),
        password_hash: HASH.to_owned(),
        workspace_id: "ws".to_owned(),
        workspace_slug: "ws".to_owned(),
        workspace_name: "Workspace".to_owned(),
    };
    store
        .create_first_user(&first, &session("first"))
        .await
        .unwrap();
    (store, dir)
}

fn session(token: &str) -> NewSession {
    NewSession {
        token_hash: token.to_owned(),
        user_id: "ada".to_owned(),
        now: 1,
        expires_at: i64::MAX,
    }
}

/// What a login read before its hash: no factor at all.
async fn read_without_app(store: &SqliteStore) {
    let found = store.credentials(ADA).await.unwrap().unwrap();
    assert!(found.totp_secret.is_none());
}

/// The owner turns the app on, as their own session can.
async fn app_on(store: &SqliteStore) {
    let basis = UpdateBasis {
        session: "first",
        checked_hash: Some(HASH),
    };
    store.start_totp("ada", b"secret", &basis, 2).await.unwrap();
    store
        .enable_totp("ada", b"secret", 1, &[], "first", 3)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_login_with_no_factor_opens_nothing_once_the_app_went_on_meanwhile() {
    let (store, _dir) = with_ada().await;
    read_without_app(&store).await;
    app_on(&store).await;
    let opened = store.create_session(&session("late"), HASH, false).await;
    assert!(!opened.unwrap());
}

#[tokio::test]
async fn a_login_with_a_mailed_code_opens_nothing_once_the_app_went_on_meanwhile() {
    let (store, _dir) = with_ada().await;
    read_without_app(&store).await;
    store
        .create_email_code(&NewEmailCode {
            purpose: CodePurpose::Login,
            email: ADA.to_owned(),
            user_id: Some("ada".to_owned()),
            code_hash: "code".to_owned(),
            now: 1,
            expires_at: i64::MAX,
        })
        .await
        .unwrap();
    app_on(&store).await;
    let attempt = CodeAttempt {
        purpose: CodePurpose::Login,
        email: ADA,
        code_hash: "code",
        now: 4,
    };
    let opened = store
        .create_session_with_code(&session("late"), HASH, &attempt)
        .await;
    assert!(!opened.unwrap());
}

#[tokio::test]
async fn a_login_with_no_factor_opens_nothing_once_the_mail_factor_went_on_meanwhile() {
    let (store, dir) = with_ada().await;
    read_without_app(&store).await;
    // As `enable_email_2fa` leaves it, without the code it needs.
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("cadocs.db"));
    let db = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("UPDATE users SET email_2fa = 1")
        .execute(&db)
        .await
        .unwrap();
    db.close().await;
    // With a relay the factor counts; without one it is not asked (ADR 0026).
    let opened = store.create_session(&session("late"), HASH, true).await;
    assert!(!opened.unwrap());
    let opened = store
        .create_session(&session("no relay"), HASH, false)
        .await;
    assert!(opened.unwrap());
}
