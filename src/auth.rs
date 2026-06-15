use std::future::{ready, Ready};

use actix_web::dev::Payload;
use actix_web::{FromRequest, HttpRequest};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde_json::Value;

use crate::di::Di;
use crate::error::CacheError;

pub struct JwtConfig {
    pub secret: Option<String>,
    pub public_key: Option<String>,
    pub jwks_url: Option<String>,
    pub role_claim: String,
}

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
    pub fn from_config(config: &JwtConfig) -> Result<Option<Verifier>, CacheError> {
        let provided = [
            config.secret.is_some(),
            config.public_key.is_some(),
            config.jwks_url.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count();
        if provided > 1 {
            return Err(CacheError::Config(
                "set only one of --jwt-secret / --jwt-public-key / --jwt-jwks-url".to_string(),
            ));
        }
        let role_claim = config.role_claim.clone();

        if let Some(secret) = &config.secret {
            return Ok(Some(Verifier {
                key: DecodingKey::from_secret(secret.as_bytes()),
                validation: pinned(Algorithm::HS256),
                role_claim,
            }));
        }
        if let Some(pem) = &config.public_key {
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
        if config.jwks_url.is_some() {
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
    let token = header
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| CacheError::Unauthorized("malformed Authorization header".to_string()))?;
    match Di::instance().verifier() {
        Some(verifier) => Ok(Some(verifier.verify(token)?)),
        None => Err(CacheError::Unauthorized(
            "a token was presented but JWT verification is not configured".to_string(),
        )),
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
        Verifier::from_config(&JwtConfig {
            secret: Some("test-secret".into()),
            public_key: None,
            jwks_url: None,
            role_claim: "role".into(),
        })
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
        assert!(hs_verifier().verify(&token).is_err());
    }

    #[test]
    fn rejects_expired_token() {
        let token = sign(
            "test-secret",
            json!({ "role": "authenticated", "exp": now_secs() - 3600 }),
        );
        assert!(hs_verifier().verify(&token).is_err());
    }

    #[test]
    fn rejects_missing_role_claim() {
        let token = sign(
            "test-secret",
            json!({ "sub": "u_1", "exp": now_secs() + 3600 }),
        );
        assert!(hs_verifier().verify(&token).is_err());
    }

    #[test]
    fn from_config_rejects_multiple_key_sources() {
        let config = JwtConfig {
            secret: Some("s".into()),
            public_key: Some("p".into()),
            jwks_url: None,
            role_claim: "role".into(),
        };
        assert!(Verifier::from_config(&config).is_err());
    }

    #[test]
    fn from_config_no_key_is_anonymous_only() {
        let config = JwtConfig {
            secret: None,
            public_key: None,
            jwks_url: None,
            role_claim: "role".into(),
        };
        assert!(Verifier::from_config(&config).unwrap().is_none());
    }
}
