//! Cognito access-token verification.
//!
//! `JwksVerifier` lazily fetches the user pool's JWKS, caches the decoding keys by `kid`
//! (the cache lives as long as the process, so it survives warm Lambda invocations) and
//! refetches when it sees an unknown `kid` (at most once per `REFETCH_COOLDOWN`).

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use crate::{AppState, error::ApiError};

/// Minimum time between two JWKS refetches triggered by unknown `kid`s.
const REFETCH_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("unsupported token algorithm")]
    WrongAlgorithm,
    #[error("unknown signing key")]
    UnknownKey,
    #[error("invalid token: {0}")]
    Invalid(String),
    #[error("token_use is not \"access\"")]
    WrongTokenUse,
    #[error("token was issued for a different client")]
    WrongClient,
    #[error("failed to fetch JWKS: {0}")]
    Jwks(String),
}

/// Claims we care about in a Cognito access token.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub token_use: Option<String>,
    pub client_id: Option<String>,
}

pub struct JwksVerifier {
    issuer: String,
    client_id: String,
    /// `None` = static key set (tests); never refetched.
    jwks_url: Option<String>,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    last_fetch: Mutex<Option<Instant>>,
}

impl JwksVerifier {
    /// Verifier for a Cognito user pool. The JWKS is fetched on first use.
    pub fn new(region: &str, user_pool_id: &str, client_id: &str) -> Self {
        let issuer = format!("https://cognito-idp.{region}.amazonaws.com/{user_pool_id}");
        let jwks_url = format!("{issuer}/.well-known/jwks.json");
        Self {
            issuer,
            client_id: client_id.to_owned(),
            jwks_url: Some(jwks_url),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            keys: RwLock::new(HashMap::new()),
            last_fetch: Mutex::new(None),
        }
    }

    /// Verifier with a fixed key set that is never refetched (for tests).
    pub fn from_static(issuer: &str, client_id: &str, jwks: &JwkSet) -> Result<Self, AuthError> {
        Ok(Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            jwks_url: None,
            http: reqwest::Client::new(),
            keys: RwLock::new(keys_from_set(jwks)?),
            last_fetch: Mutex::new(None),
        })
    }

    /// Verify a raw (no `Bearer ` prefix) access token and return its claims.
    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError::Malformed(e.to_string()))?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::WrongAlgorithm);
        }
        let kid = header
            .kid
            .ok_or_else(|| AuthError::Malformed("missing kid".into()))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);
        // Cognito access tokens carry `client_id`, not `aud`; checked manually below.
        validation.validate_aud = false;

        if !self.keys.read().await.contains_key(&kid) {
            self.refresh_for_unknown_kid().await?;
        }
        let claims = {
            let keys = self.keys.read().await;
            let key = keys.get(&kid).ok_or(AuthError::UnknownKey)?;
            decode::<Claims>(token, key, &validation)
                .map_err(|e| AuthError::Invalid(e.to_string()))?
                .claims
        };

        if claims.token_use.as_deref() != Some("access") {
            return Err(AuthError::WrongTokenUse);
        }
        if claims.client_id.as_deref() != Some(self.client_id.as_str()) {
            return Err(AuthError::WrongClient);
        }
        Ok(claims)
    }

    async fn refresh_for_unknown_kid(&self) -> Result<(), AuthError> {
        let Some(url) = &self.jwks_url else {
            return Err(AuthError::UnknownKey);
        };
        // Serialize refetches; skip if one happened recently (another request may have
        // just refreshed the cache, or someone is spraying random kids).
        let mut last = self.last_fetch.lock().await;
        if last.is_some_and(|t| t.elapsed() < REFETCH_COOLDOWN) {
            return Ok(());
        }
        let jwks: JwkSet = self
            .http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AuthError::Jwks(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::Jwks(e.to_string()))?;
        let fresh = keys_from_set(&jwks)?;
        tracing::info!(keys = fresh.len(), "fetched Cognito JWKS");
        *self.keys.write().await = fresh;
        *last = Some(Instant::now());
        Ok(())
    }
}

fn keys_from_set(jwks: &JwkSet) -> Result<HashMap<String, DecodingKey>, AuthError> {
    let mut out = HashMap::new();
    for jwk in &jwks.keys {
        let Some(kid) = &jwk.common.key_id else {
            continue;
        };
        match DecodingKey::from_jwk(jwk) {
            Ok(key) => {
                out.insert(kid.clone(), key);
            }
            Err(e) => tracing::warn!(kid, error = %e, "skipping unusable JWK"),
        }
    }
    if out.is_empty() {
        return Err(AuthError::Jwks("JWKS contains no usable keys".into()));
    }
    Ok(out)
}

/// The authenticated caller. `sub` is the Cognito user id and the only source of `pk`.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub sub: String,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;

        match state.auth.verify(token).await {
            Ok(claims) => Ok(AuthUser { sub: claims.sub }),
            Err(AuthError::Jwks(e)) => Err(ApiError::internal(format!("JWKS unavailable: {e}"))),
            Err(e) => {
                tracing::debug!(error = %e, "rejected token");
                Err(ApiError::Unauthorized("invalid token".into()))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};

    pub const TEST_ISSUER: &str = "https://cognito-idp.us-west-2.amazonaws.com/us-west-2_test";
    pub const TEST_CLIENT_ID: &str = "test-client";
    pub const TEST_KID: &str = "test-kid";
    const PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    const JWKS_JSON: &str = include_str!("../tests/fixtures/test_jwks.json");

    pub fn static_verifier() -> JwksVerifier {
        let jwks: JwkSet = serde_json::from_str(JWKS_JSON).unwrap();
        JwksVerifier::from_static(TEST_ISSUER, TEST_CLIENT_ID, &jwks).unwrap()
    }

    pub fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    pub fn valid_claims(sub: &str) -> Value {
        json!({
            "sub": sub,
            "iss": TEST_ISSUER,
            "client_id": TEST_CLIENT_ID,
            "token_use": "access",
            "scope": "aws.cognito.signin.user.admin",
            "iat": now(),
            "exp": now() + 3600,
        })
    }

    pub fn sign_with_kid(claims: &Value, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_owned());
        let key = EncodingKey::from_rsa_pem(PRIVATE_PEM).unwrap();
        encode(&header, claims, &key).unwrap()
    }

    pub fn sign(claims: &Value) -> String {
        sign_with_kid(claims, TEST_KID)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    #[tokio::test]
    async fn accepts_valid_access_token() {
        let v = static_verifier();
        let claims = v.verify(&sign(&valid_claims("user-1"))).await.unwrap();
        assert_eq!(claims.sub, "user-1");
    }

    #[tokio::test]
    async fn rejects_id_token() {
        let mut c = valid_claims("u");
        c["token_use"] = "id".into();
        let err = static_verifier().verify(&sign(&c)).await.unwrap_err();
        assert!(matches!(err, AuthError::WrongTokenUse), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_wrong_client_id() {
        let mut c = valid_claims("u");
        c["client_id"] = "someone-else".into();
        let err = static_verifier().verify(&sign(&c)).await.unwrap_err();
        assert!(matches!(err, AuthError::WrongClient), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_missing_client_id() {
        let mut c = valid_claims("u");
        c.as_object_mut().unwrap().remove("client_id");
        let err = static_verifier().verify(&sign(&c)).await.unwrap_err();
        assert!(matches!(err, AuthError::WrongClient), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_expired() {
        let mut c = valid_claims("u");
        c["exp"] = (now() - 3600).into();
        let err = static_verifier().verify(&sign(&c)).await.unwrap_err();
        assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_bad_issuer() {
        let mut c = valid_claims("u");
        c["iss"] = "https://cognito-idp.us-west-2.amazonaws.com/other_pool".into();
        let err = static_verifier().verify(&sign(&c)).await.unwrap_err();
        assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_unknown_kid_with_static_set() {
        let err = static_verifier()
            .verify(&sign_with_kid(&valid_claims("u"), "other-kid"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::UnknownKey), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_hs256() {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(TEST_KID.into());
        let token = encode(
            &header,
            &valid_claims("u"),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        let err = static_verifier().verify(&token).await.unwrap_err();
        assert!(matches!(err, AuthError::WrongAlgorithm), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_tampered_signature() {
        let token = sign(&valid_claims("u"));
        let mut tampered = token[..token.len() - 4].to_owned();
        tampered.push_str("AAAA");
        let err = static_verifier().verify(&tampered).await.unwrap_err();
        assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_garbage() {
        let err = static_verifier().verify("not-a-jwt").await.unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)), "{err:?}");
    }
}
