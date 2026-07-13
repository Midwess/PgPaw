use std::future::{ready, Ready};

use actix_web::dev::Payload;
use actix_web::{FromRequest, HttpRequest};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde_json::Value;

use crate::di::Di;
use crate::error::CacheError;

#[derive(Clone)]
pub struct Principal {
    pub role: String,
    pub claims_json: String,
}

pub struct Verifier {
    key: DecodingKey,
    validation: Validation,
    role_claim: String,
}

impl Verifier {
    pub fn build(
        secret: Option<String>,
        public_key: Option<String>,
        jwks_url: Option<String>,
        role_claim: String,
    ) -> Result<Option<Verifier>, CacheError> {
        let provided = [secret.is_some(), public_key.is_some(), jwks_url.is_some()]
            .iter()
            .filter(|set| **set)
            .count();
        if provided > 1 {
            return Err(CacheError::Config(
                "set only one of --jwt-secret / --jwt-public-key / --jwt-jwks-url".to_string(),
            ));
        }

        if let Some(secret) = secret {
            return Ok(Some(Verifier {
                key: DecodingKey::from_secret(secret.as_bytes()),
                validation: pinned(Algorithm::HS256),
                role_claim,
            }));
        }
        if let Some(pem) = public_key {
            let (key, alg) = DecodingKey::from_rsa_pem(pem.as_bytes())
                .map(|key| (key, Algorithm::RS256))
                .or_else(|_| {
                    DecodingKey::from_ec_pem(pem.as_bytes()).map(|key| (key, Algorithm::ES256))
                })
                .map_err(|e| CacheError::Config(format!("invalid JWT public key PEM: {e}")))?;
            return Ok(Some(Verifier {
                key,
                validation: pinned(alg),
                role_claim,
            }));
        }
        if jwks_url.is_some() {
            return Err(CacheError::Config(
                "JWKS URL verification is not yet implemented; use --jwt-secret or --jwt-public-key"
                    .to_string(),
            ));
        }
        Ok(None)
    }

    pub fn verify(&self, token: &str) -> Result<Principal, CacheError> {
        let data = decode::<Value>(token, &self.key, &self.validation)
            .map_err(|e| CacheError::Unauthorized(format!("invalid token: {e}")))?;
        let role = data
            .claims
            .get(&self.role_claim)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CacheError::Unauthorized(format!(
                    "token missing string '{}' claim",
                    self.role_claim
                ))
            })?
            .to_string();
        Ok(Principal {
            role,
            claims_json: data.claims.to_string(),
        })
    }
}

fn pinned(alg: Algorithm) -> Validation {
    let mut validation = Validation::new(alg);
    validation.validate_aud = false;
    validation
}

pub struct AuthOutcome(pub Result<Option<Principal>, CacheError>);

impl FromRequest for AuthOutcome {
    type Error = actix_web::Error;
    type Future = Ready<Result<AuthOutcome, actix_web::Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        ready(Ok(AuthOutcome(authenticate(req))))
    }
}

fn authenticate(req: &HttpRequest) -> Result<Option<Principal>, CacheError> {
    let Some(header) = req.headers().get("Authorization") else {
        return Ok(None);
    };
    log::info!("event=auth_header_present");
    let token = header
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| {
            log::warn!("event=auth_failed reason=malformed_authorization_header");
            CacheError::Unauthorized("malformed Authorization header".to_string())
        })?;
    match Di::instance().operations().authenticate(Some(token)) {
            Ok(Some(principal)) => {
                log::info!("event=auth_verified role={}", principal.role);
                Ok(Some(principal))
            }
            Err(error) => {
                log::warn!("event=auth_failed error={:?}", error.to_string());
                Err(error)
            }
            Ok(None) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn hs_verifier() -> Verifier {
        Verifier::build(Some("test-secret".into()), None, None, "role".into())
            .unwrap()
            .unwrap()
    }

    fn sign(secret: &str, claims: Value) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn hs256_valid_token_yields_principal() {
        let token = sign(
            "test-secret",
            json!({ "role": "authenticated", "org_id": 7, "exp": now_secs() + 3600 }),
        );
        let principal = hs_verifier().verify(&token).unwrap();
        assert_eq!(principal.role, "authenticated");
        assert!(principal.claims_json.contains("\"org_id\":7"));
    }

    #[test]
    fn rejects_bad_signature() {
        let token = sign(
            "wrong-secret",
            json!({ "role": "authenticated", "exp": now_secs() + 3600 }),
        );
        assert!(matches!(
            hs_verifier().verify(&token),
            Err(CacheError::Unauthorized(_))
        ));
    }

    #[test]
    fn rejects_expired_token() {
        let token = sign(
            "test-secret",
            json!({ "role": "authenticated", "exp": now_secs() - 3600 }),
        );
        assert!(matches!(
            hs_verifier().verify(&token),
            Err(CacheError::Unauthorized(_))
        ));
    }

    #[test]
    fn rejects_missing_role_claim() {
        let token = sign(
            "test-secret",
            json!({ "sub": "u_1", "exp": now_secs() + 3600 }),
        );
        assert!(matches!(
            hs_verifier().verify(&token),
            Err(CacheError::Unauthorized(message)) if message.contains("missing string 'role'")
        ));
    }

    #[test]
    fn build_rejects_multiple_key_sources() {
        assert!(Verifier::build(Some("s".into()), Some("p".into()), None, "role".into()).is_err());
    }

    #[test]
    fn build_no_key_is_anonymous_only() {
        assert!(Verifier::build(None, None, None, "role".into())
            .unwrap()
            .is_none());
    }
}
