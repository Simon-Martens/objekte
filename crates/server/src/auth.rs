use crate::error::{Result, WebError};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::http::{HeaderMap, StatusCode};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Auth {
    inner: Arc<AuthStorage>,
    scope: String,
    pub insecure_cookie: bool,
}
struct AuthStorage {
    _db: turso::Database,
    conn: Mutex<turso::Connection>,
    attempts: Mutex<HashMap<String, (u32, i64)>>,
}
#[derive(Clone, Debug)]
pub struct Actor {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub csrf: String,
    pub session: String,
}
impl Actor {
    pub fn editable(&self) -> bool {
        self.role == "editor" || self.role == "administrator"
    }
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn secret() -> String {
    let mut bytes = [0; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn unauthorized() -> WebError {
    WebError(StatusCode::UNAUTHORIZED, "Please sign in.".into())
}
impl Auth {
    pub fn scoped(&self, project: &str) -> Self {
        Self {
            scope: project.into(),
            ..self.clone()
        }
    }
    pub async fn open(path: &Path, insecure_cookie: bool) -> Result<Self> {
        let db = turso::Builder::new_local(
            path.to_str()
                .ok_or_else(|| WebError::bad("Invalid database path"))?,
        )
        .build()
        .await?;
        let conn = db.connect()?;
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        let version: i64 = conn
            .query("PRAGMA user_version", ())
            .await?
            .next()
            .await?
            .ok_or_else(|| WebError::internal("Missing database version"))?
            .get(0)?;
        if version == 0 {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;\n{}\nCOMMIT;",
                include_str!("../migrations/001_admin.sql")
            ))
            .await?;
        } else if version != 1 && version != 2 {
            return Err(WebError::bad(
                "Unsupported admin database version; use a separate database for the new server.",
            ));
        }
        if version < 2 {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;\n{}\nCOMMIT;",
                include_str!("../migrations/002_accounts.sql")
            ))
            .await?;
        }
        Ok(Self {
            inner: Arc::new(AuthStorage {
                _db: db,
                conn: Mutex::new(conn),
                attempts: Mutex::new(HashMap::new()),
            }),
            scope: String::new(),
            insecure_cookie,
        })
    }
    pub async fn create_admin(&self, email: &str, name: &str, password: String) -> Result<()> {
        let email = email.trim().to_lowercase();
        if !email.contains('@') || name.trim().is_empty() || password.len() < 12 {
            return Err(WebError::bad(
                "Provide an email, name and password of at least 12 characters.",
            ));
        }
        let hash = tokio::task::spawn_blocking(move || {
            Argon2::default()
                .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
                .map(|v| v.to_string())
        })
        .await
        .map_err(WebError::internal)?
        .map_err(WebError::internal)?;
        let conn = self.inner.conn.lock().await;
        let count: i64 = conn
            .query("SELECT count(*) FROM users WHERE role='administrator'", ())
            .await?
            .next()
            .await?
            .unwrap()
            .get(0)?;
        if count > 0 {
            return Err(WebError::bad("An administrator already exists."));
        }
        conn.execute("INSERT INTO users(email,display_name,password_hash,role) VALUES(?1,?2,?3,'administrator')", [email.as_str(), name.trim(), &hash]).await?;
        Ok(())
    }
    pub async fn login(&self, email: &str, password: String) -> Result<String> {
        let email = email.trim().to_lowercase();
        {
            let mut attempts = self.inner.attempts.lock().await;
            attempts.retain(|_, (_, started)| now() - *started < 300);
            if attempts.len() >= 4096 && !attempts.contains_key(&email) {
                return Err(WebError(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Try again later.".into(),
                ));
            }
            let entry = attempts.entry(email.clone()).or_insert((0, now()));
            entry.0 += 1;
            if entry.0 > 10 {
                return Err(WebError(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Try again later.".into(),
                ));
            }
        }
        let record = {
            let conn = self.inner.conn.lock().await;
            let mut rows = conn
                .query(
                    "SELECT id,password_hash FROM users WHERE email=?1 AND disabled=0",
                    [email.as_str()],
                )
                .await?;
            rows.next()
                .await?
                .map(|r| Ok::<_, turso::Error>((r.get::<i64>(0)?, r.get::<String>(1)?)))
                .transpose()?
        };
        let Some((id, hash)) = record else {
            return Err(unauthorized());
        };
        let valid = tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            })
        })
        .await
        .map_err(WebError::internal)?;
        if !valid {
            return Err(unauthorized());
        }
        let token = secret();
        let csrf = secret();
        let conn = self.inner.conn.lock().await;
        conn.execute("DELETE FROM sessions WHERE expires_at < ?1", [now()])
            .await?;
        conn.execute(
            "INSERT INTO sessions(token_hash,user_id,csrf,expires_at) VALUES(?1,?2,?3,?4)",
            turso::params![digest(&token), id, csrf, now() + 7 * 86400],
        )
        .await?;
        self.inner.attempts.lock().await.remove(&email);
        Ok(token)
    }
    pub async fn actor(&self, headers: &HeaderMap) -> Result<Actor> {
        let cookie = headers
            .get("cookie")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(unauthorized)?;
        let token = cookie
            .split(';')
            .map(str::trim)
            .find_map(|s| s.strip_prefix("objekte_session="))
            .ok_or_else(unauthorized)?;
        let session = digest(token);
        let conn = self.inner.conn.lock().await;
        let mut rows = conn.query("SELECT u.id,u.display_name,u.role,s.csrf FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=?1 AND s.expires_at>?2 AND u.disabled=0", turso::params![session.clone(), now()]).await?;
        let row = rows.next().await?.ok_or_else(unauthorized)?;
        Ok(Actor {
            id: row.get(0)?,
            name: row.get(1)?,
            role: row.get(2)?,
            csrf: digest(&format!("{}:{}", row.get::<String>(3)?, self.scope)),
            session,
        })
    }
    pub fn mutation(&self, actor: &Actor, csrf: &str, edit: bool) -> Result<()> {
        if actor.csrf != csrf || (edit && !actor.editable()) {
            return Err(WebError(
                StatusCode::FORBIDDEN,
                "Permission denied or invalid CSRF token.".into(),
            ));
        }
        Ok(())
    }
    pub async fn logout(&self, actor: &Actor) -> Result<()> {
        self.inner.conn
            .lock()
            .await
            .execute(
                "DELETE FROM sessions WHERE token_hash=?1",
                [actor.session.as_str()],
            )
            .await?;
        Ok(())
    }
    pub fn cookie(&self, token: &str) -> String {
        format!(
            "objekte_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
            if token.is_empty() { 0 } else { 7 * 86400 },
            if self.insecure_cookie { "" } else { "; Secure" }
        )
    }
}

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub role: String,
    pub disabled: bool,
}
#[derive(Clone)]
pub struct Invitation {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub expires: i64,
    pub used: bool,
}
fn email(value: &str) -> Result<String> {
    let value = value.trim().to_lowercase();
    if value.len() > 254
        || value.chars().any(char::is_whitespace)
        || value.split('@').count() != 2
        || value.starts_with('@')
        || value.ends_with('@')
    {
        Err(WebError::bad("Enter a valid email address"))
    } else {
        Ok(value)
    }
}
fn role(value: &str) -> Result<()> {
    if ["user", "editor", "administrator"].contains(&value) {
        Ok(())
    } else {
        Err(WebError::bad("Unknown role"))
    }
}
async fn password_hash(password: String) -> Result<String> {
    if password.len() < 12 {
        return Err(WebError::bad(
            "Password must contain at least 12 characters",
        ));
    }
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|s| s.to_string())
    })
    .await
    .map_err(WebError::internal)?
    .map_err(WebError::internal)
}
async fn verify(password: String, hash: String) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
    })
    .await
    .map_err(WebError::internal)
}
impl Auth {
    pub fn administrator(&self, actor: &Actor) -> Result<()> {
        if actor.role == "administrator" {
            Ok(())
        } else {
            Err(WebError(
                axum::http::StatusCode::FORBIDDEN,
                "Administrator permission required".into(),
            ))
        }
    }
    pub async fn users(&self) -> Result<Vec<User>> {
        let conn = self.inner.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id,email,display_name,role,disabled FROM users ORDER BY email",
                (),
            )
            .await?;
        let mut users = Vec::new();
        while let Some(r) = rows.next().await? {
            users.push(User {
                id: r.get(0)?,
                email: r.get(1)?,
                name: r.get(2)?,
                role: r.get(3)?,
                disabled: r.get::<i64>(4)? != 0,
            });
        }
        Ok(users)
    }
    pub async fn set_user(&self, id: i64, new_role: &str, disabled: bool) -> Result<()> {
        role(new_role)?;
        let conn = self.inner.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let r = conn
                .query("SELECT role,disabled FROM users WHERE id=?1", [id])
                .await?
                .next()
                .await?
                .ok_or_else(|| WebError::bad("User missing"))?;
            if r.get::<String>(0)? == "administrator"
                && r.get::<i64>(1)? == 0
                && (disabled || new_role != "administrator")
            {
                let count: i64 = conn
                    .query(
                        "SELECT count(*) FROM users WHERE role='administrator' AND disabled=0",
                        (),
                    )
                    .await?
                    .next()
                    .await?
                    .unwrap()
                    .get(0)?;
                if count <= 1 {
                    return Err(WebError::bad(
                        "Cannot remove the final active administrator",
                    ));
                }
            }
            conn.execute(
                "UPDATE users SET role=?1,disabled=?2 WHERE id=?3",
                turso::params![new_role, if disabled { 1 } else { 0 }, id],
            )
            .await?;
            conn.execute("DELETE FROM sessions WHERE user_id=?1", [id])
                .await?;
            Ok(())
        }
        .await;
        conn.execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" }, ())
            .await?;
        result
    }
    pub async fn revoke_sessions(&self, id: i64) -> Result<()> {
        self.inner.conn
            .lock()
            .await
            .execute("DELETE FROM sessions WHERE user_id=?1", [id])
            .await?;
        Ok(())
    }
    pub async fn invite(&self, address: &str, new_role: &str) -> Result<String> {
        role(new_role)?;
        let address = email(address)?;
        let token = secret();
        self.inner.conn
            .lock()
            .await
            .execute(
                "INSERT INTO invitations(token_hash,email,role,expires_at) VALUES(?1,?2,?3,?4)",
                turso::params![digest(&token), address, new_role, now() + 7 * 86400],
            )
            .await?;
        Ok(token)
    }
    pub async fn invitations(&self) -> Result<Vec<Invitation>> {
        let conn = self.inner.conn.lock().await;
        let mut rows=conn.query("SELECT id,email,role,expires_at,redeemed,revoked FROM invitations ORDER BY id DESC",()).await?;
        let mut result = Vec::new();
        while let Some(r) = rows.next().await? {
            result.push(Invitation {
                id: r.get(0)?,
                email: r.get(1)?,
                role: r.get(2)?,
                expires: r.get(3)?,
                used: r.get::<i64>(4)? != 0 || r.get::<i64>(5)? != 0,
            });
        }
        Ok(result)
    }
    pub async fn revoke_invitation(&self, id: i64) -> Result<()> {
        self.inner.conn
            .lock()
            .await
            .execute("UPDATE invitations SET revoked=1 WHERE id=?1", [id])
            .await?;
        Ok(())
    }
    pub async fn redeem_invitation(
        &self,
        token: &str,
        address: &str,
        name: &str,
        password: String,
    ) -> Result<()> {
        let address = email(address)?;
        if name.trim().is_empty() {
            return Err(WebError::bad("Display name required"));
        }
        let hash = password_hash(password).await?;
        let conn = self.inner.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result=async{let r=conn.query("SELECT id,email,role FROM invitations WHERE token_hash=?1 AND expires_at>?2 AND redeemed=0 AND revoked=0",turso::params![digest(token),now()]).await?.next().await?.ok_or_else(||WebError::bad("Invitation invalid or expired"))?;if r.get::<String>(1)?!=address{return Err(WebError::bad("Email does not match this invitation"));}if conn.query("SELECT id FROM users WHERE email=?1",[address.as_str()]).await?.next().await?.is_some(){return Err(WebError::bad("Email already registered"));}
 conn.execute("INSERT INTO users(email,display_name,password_hash,role) VALUES(?1,?2,?3,?4)",turso::params![address,name.trim(),hash,r.get::<String>(2)?]).await?;conn.execute("UPDATE invitations SET redeemed=1 WHERE id=?1",[r.get::<i64>(0)?]).await?;Ok(())}.await;
        conn.execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" }, ())
            .await?;
        result
    }
    pub async fn create_reset(&self, id: i64) -> Result<String> {
        let token = secret();
        let conn = self.inner.conn.lock().await;
        if conn
            .query("SELECT id FROM users WHERE id=?1 AND disabled=0", [id])
            .await?
            .next()
            .await?
            .is_none()
        {
            return Err(WebError::bad("Active user missing"));
        }
        conn.execute("UPDATE resets SET used=1 WHERE user_id=?1", [id])
            .await?;
        conn.execute(
            "INSERT INTO resets(token_hash,user_id,expires_at) VALUES(?1,?2,?3)",
            turso::params![digest(&token), id, now() + 3600],
        )
        .await?;
        Ok(token)
    }
    pub async fn redeem_reset(&self, token: &str, password: String) -> Result<()> {
        let hash = password_hash(password).await?;
        let conn = self.inner.conn.lock().await;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result=async{let row=conn.query("SELECT r.user_id FROM resets r JOIN users u ON u.id=r.user_id WHERE r.token_hash=?1 AND r.used=0 AND r.expires_at>?2 AND u.disabled=0",turso::params![digest(token),now()]).await?.next().await?.ok_or_else(||WebError::bad("Reset link invalid or expired"))?;let id:i64=row.get(0)?;conn.execute("UPDATE users SET password_hash=?1 WHERE id=?2",turso::params![hash,id]).await?;conn.execute("DELETE FROM sessions WHERE user_id=?1",[id]).await?;conn.execute("UPDATE resets SET used=1 WHERE user_id=?1",[id]).await?;Ok(())}.await;
        conn.execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" }, ())
            .await?;
        result
    }
    pub async fn change_password(&self, id: i64, current: String, new: String) -> Result<()> {
        let conn = self.inner.conn.lock().await;
        let old: String = conn
            .query("SELECT password_hash FROM users WHERE id=?1", [id])
            .await?
            .next()
            .await?
            .ok_or_else(unauthorized)?
            .get(0)?;
        if !verify(current, old).await? {
            return Err(unauthorized());
        }
        let hash = password_hash(new).await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            conn.execute(
                "UPDATE users SET password_hash=?1 WHERE id=?2",
                turso::params![hash, id],
            )
            .await?;
            conn.execute("DELETE FROM sessions WHERE user_id=?1", [id])
                .await?;
            conn.execute("UPDATE resets SET used=1 WHERE user_id=?1", [id])
                .await?;
            Ok(())
        }
        .await;
        conn.execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" }, ())
            .await?;
        result
    }
    pub async fn profile(&self, id: i64, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(WebError::bad("Display name required"));
        }
        self.inner.conn
            .lock()
            .await
            .execute(
                "UPDATE users SET display_name=?1 WHERE id=?2",
                turso::params![name.trim(), id],
            )
            .await?;
        Ok(())
    }
}
