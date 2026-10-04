use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::{AuthProvider, Settings};
use crate::http::{Header, Request};
use crate::secret::Secret;

const CLOCK_SKEW_SECONDS: i64 = 30;
const MAX_TOKEN_BYTES: usize = 8 * 1024;
const MAX_JWT_PART_BYTES: usize = 6 * 1024;
const MAX_IDENTITY_TEXT_BYTES: usize = 256;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub mod store;

tokio::task_local! { static INVOCATION: Invocation; }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claims {
    pub subject: String,
    pub session: String,
    pub tenant: Option<String>,
    pub site: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub id: String,
    pub user_id: Option<i64>,
    pub tenant_id: Option<i64>,
}

pub struct Prepared {
    site: String,
    provider_key: String,
    provider: AuthProvider,
    origin: Option<crate::config::HttpOrigin>,
    claims: Option<Claims>,
}

impl Prepared {
    pub fn claims(&self) -> Option<&Claims> {
        self.claims.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Unauthorized,
    Forbidden,
    Internal,
}

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Internal => "authentication configuration error",
        })
    }
}

struct Invocation {
    site: String,
    provider_key: String,
    provider: AuthProvider,
    origin: Option<crate::config::HttpOrigin>,
    session: Option<String>,
    identity: Option<Identity>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JwtHeader {
    alg: String,
    typ: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct JwtClaims {
    iss: String,
    sub: String,
    aud: String,
    sid: String,
    tenant: Option<String>,
    iat: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    nbf: Option<i64>,
    exp: i64,
}

#[derive(Serialize)]
struct IssuedHeader {
    alg: &'static str,
    typ: &'static str,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CredentialKind {
    Authorization,
    Cookie,
}

pub fn prepare(directory: &[&str], request: &Request, anonymous: bool) -> Result<Prepared, Error> {
    prepare_with_settings(
        &crate::config::current_settings(),
        directory,
        request,
        anonymous,
    )
}

pub fn prepare_with_settings(
    settings: &Settings,
    directory: &[&str],
    request: &Request,
    anonymous: bool,
) -> Result<Prepared, Error> {
    let (site_key, site) = settings
        .site_for_directory(directory)
        .map_err(|_| Error::Internal)?
        .ok_or(Error::Internal)?;
    let provider = settings
        .auth()
        .provider(site.auth())
        .ok_or(Error::Internal)?;
    validate_request_host(request, site.hosts())?;

    let authorization = bearer_token(request)?;
    // Public entries are anonymous for browser Cookie credentials. Expired or
    // revoked sessions therefore cannot prevent a fresh login. An explicit
    // Authorization credential remains strict.
    let cookie = if anonymous {
        None
    } else {
        cookie_token(request, provider.cookie())?
    };
    let credential = match (authorization, cookie) {
        (Some(_), Some(_)) => return Err(Error::Unauthorized),
        (Some(token), None) => Some((CredentialKind::Authorization, token)),
        (None, Some(token)) => Some((CredentialKind::Cookie, token)),
        (None, None) if anonymous => None,
        (None, None) => return Err(Error::Unauthorized),
    };
    if credential
        .as_ref()
        .is_some_and(|(kind, _)| *kind == CredentialKind::Cookie)
    {
        validate_cookie_origin(request, site.origin())?;
    }
    let claims = credential
        .map(|(_, token)| verify_jwt(&token, site_key, site.auth(), provider))
        .transpose()?;
    Ok(Prepared {
        site: site_key.to_owned(),
        provider_key: site.auth().to_owned(),
        provider: provider.clone(),
        origin: site.origin().cloned(),
        claims,
    })
}

pub async fn scope<F>(
    prepared: Prepared,
    identity: Option<Identity>,
    future: F,
) -> Result<F::Output, Error>
where
    F: Future,
{
    validate_identity(prepared.claims.as_ref(), identity.as_ref())?;
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    let execution = prepared
        .claims
        .as_ref()
        .zip(identity.as_ref())
        .map(|(claims, identity)| crate::job::ExecutionIdentity::User {
            tenant_id: identity.tenant_id,
            provider: prepared.provider_key.clone(),
            site: prepared.site.clone(),
            subject: claims.subject.clone(),
            session: claims.session.clone(),
            tenant_claim: claims.tenant.clone(),
            permission_key: String::new(),
        });
    let session = prepared
        .claims
        .as_ref()
        .map(|claims| claims.session.clone());
    let invocation = Invocation {
        site: prepared.site,
        provider_key: prepared.provider_key,
        provider: prepared.provider,
        origin: prepared.origin,
        session,
        identity,
    };
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    if let Some(tenant_id) = invocation
        .identity
        .as_ref()
        .and_then(|identity| identity.tenant_id)
    {
        let future = crate::tenant::scope(tenant_id, future);
        return Ok(match execution {
            Some(execution) => {
                INVOCATION
                    .scope(invocation, crate::job::scope_execution(execution, future))
                    .await
            }
            None => INVOCATION.scope(invocation, future).await,
        });
    }
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    if let Some(execution) = execution {
        return Ok(INVOCATION
            .scope(invocation, crate::job::scope_execution(execution, future))
            .await);
    }
    Ok(INVOCATION.scope(invocation, future).await)
}

pub fn validate_job_source(provider_key: &str, site_key: &str) -> Result<(), Error> {
    let settings = crate::config::current_settings();
    let site = settings.sites().get(site_key).ok_or(Error::Internal)?;
    if site.auth() != provider_key || settings.auth().provider(provider_key).is_none() {
        return Err(Error::Internal);
    }
    Ok(())
}

pub fn validate_job_identity(claims: &Claims, identity: &Identity) -> Result<(), Error> {
    validate_identity(Some(claims), Some(identity))
}

pub fn site_key() -> Result<String, Error> {
    with_invocation(|invocation| Ok(invocation.site.clone()))
}

pub fn id() -> Result<String, Error> {
    with_identity(|identity| Ok(identity.id.clone()))
}

pub fn session() -> Result<String, Error> {
    with_invocation(|invocation| {
        if invocation.identity.is_none() {
            return Err(Error::Unauthorized);
        }
        invocation.session.clone().ok_or(Error::Internal)
    })
}

pub fn user_id() -> Result<Option<i64>, Error> {
    with_identity(|identity| Ok(identity.user_id))
}

pub fn tenant_id() -> Result<Option<i64>, Error> {
    with_identity(|identity| Ok(identity.tenant_id))
}

pub fn owns_user(user_id: i64) -> Result<bool, Error> {
    with_identity(|identity| Ok(identity.user_id == Some(user_id)))
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn require_components(
    components: &[&str],
    tenant_components: &[&str],
) -> Result<(), Error> {
    if crate::config::current_settings().tenant().is_none() || components.is_empty() {
        return Ok(());
    }
    let tenant = tenant_id()?.ok_or(Error::Forbidden)?;
    crate::tenant::require_components(tenant, components, tenant_components)
        .await
        .map_err(|error| {
            if error.kind() == crate::orm::ErrorKind::InvalidData {
                Error::Forbidden
            } else {
                crate::log::error(error.to_string(), Vec::new());
                Error::Internal
            }
        })
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn authorize_permission(permission: &str, site: &str) -> Result<(), Error> {
    let user = user_id()?.ok_or(Error::Forbidden)?;
    let (_, database, actual_site) = authorization_context().await.map_err(|error| {
        crate::log::error(error, Vec::new());
        Error::Internal
    })?;
    if actual_site != site {
        return Err(Error::Forbidden);
    }
    match store::authorize(database, user, site, permission).await {
        Ok(true) => crate::job::bind_permission(permission).map_err(|error| {
            crate::log::error(error.to_string(), Vec::new());
            Error::Internal
        }),
        Ok(false) => Err(Error::Forbidden),
        Err(error) => {
            crate::log::error(error.to_string(), Vec::new());
            Err(Error::Internal)
        }
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn permissions() -> Result<Vec<store::CatalogPermission>, String> {
    let (catalog, _, site) = authorization_context().await?;
    let mut permissions = store::list_permissions(catalog)
        .await
        .map_err(|error| error.to_string())?;
    permissions.retain(|permission| permission.site == site);
    Ok(permissions)
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn save_role(
    id: &str,
    name: &str,
    all_permissions: bool,
    permission_keys: Vec<String>,
) -> Result<(), String> {
    let (catalog, authorization, site) = authorization_context().await?;
    store::save_role(
        catalog,
        authorization,
        &store::Role {
            id: id.to_owned(),
            site,
            name: name.to_owned(),
            all_permissions,
        },
        &permission_keys,
    )
    .await
    .map_err(|error| error.to_string())
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn grant_role(user_id: i64, role_id: &str) -> Result<(), String> {
    let (_, authorization, site) = authorization_context().await?;
    store::grant_role(authorization, user_id, &site, role_id)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn revoke_role(user_id: i64, role_id: &str) -> Result<(), String> {
    let (_, authorization, site) = authorization_context().await?;
    store::revoke_role(authorization, user_id, &site, role_id)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub async fn disable_role(role_id: &str) -> Result<(), String> {
    let (_, authorization, site) = authorization_context().await?;
    store::disable_role(authorization, &site, role_id)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
async fn authorization_context()
-> Result<(crate::database::Database, crate::database::Database, String), String> {
    let settings = crate::config::current_settings();
    let connection = settings
        .tenant()
        .map(|tenant| tenant.database())
        .unwrap_or("default");
    let catalog =
        crate::database::database_for(Some(connection), "").map_err(|error| error.to_string())?;
    let authorization = match tenant_id().map_err(|error| error.to_string())? {
        Some(_) => crate::tenant::database(connection)
            .await
            .map_err(|error| error.to_string())?,
        None => catalog.clone(),
    };
    let site = site_key().map_err(|error| error.to_string())?;
    Ok((catalog, authorization, site))
}

pub fn issue(subject: &str, session: &str, tenant: Option<&str>) -> Result<Secret, String> {
    validate_identity_text("subject", subject).map_err(|error| error.to_string())?;
    validate_identity_text("session", session).map_err(|error| error.to_string())?;
    if let Some(tenant) = tenant {
        validate_identity_text("tenant", tenant).map_err(|error| error.to_string())?;
    }
    INVOCATION
        .try_with(|invocation| {
            let now = unix_seconds().map_err(|error| error.to_string())?;
            let ttl = i64::try_from(invocation.provider.ttl_seconds())
                .map_err(|_| Error::Internal.to_string())?;
            let claims = JwtClaims {
                iss: invocation.provider_key.clone(),
                sub: subject.to_owned(),
                aud: invocation.site.clone(),
                sid: session.to_owned(),
                tenant: tenant.map(str::to_owned),
                iat: now,
                nbf: None,
                exp: now
                    .checked_add(ttl)
                    .ok_or_else(|| Error::Internal.to_string())?,
            };
            encode_jwt(&claims, &invocation.provider)
                .map(Secret::from_input)
                .map_err(|error| error.to_string())
        })
        .map_err(|_| {
            "authentication capability is available only within a site API entry".to_owned()
        })?
}

pub fn issue_cookie(subject: &str, session: &str, tenant: Option<&str>) -> Result<(), String> {
    let token = issue(subject, session, tenant)?;
    INVOCATION
        .try_with(|invocation| {
            let max_age = i64::try_from(invocation.provider.ttl_seconds())
                .map_err(|_| "authentication configuration error".to_owned())?;
            crate::api::response_secret_cookie(
                invocation.provider.cookie(),
                token,
                session_cookie_options(Some(max_age)),
            )
        })
        .map_err(|_| {
            "authentication capability is available only within a site API entry".to_owned()
        })?
}

pub fn clear_cookie() -> Result<(), String> {
    INVOCATION
        .try_with(|invocation| {
            crate::api::response_cookie(
                invocation.provider.cookie(),
                "",
                session_cookie_options(Some(0)),
            )
        })
        .map_err(|_| {
            "authentication capability is available only within a site API entry".to_owned()
        })?
}

fn session_cookie_options(max_age: Option<i64>) -> crate::api::CookieOptions {
    crate::api::CookieOptions {
        path: "/".into(),
        domain: None,
        same_site: crate::api::SameSite::Lax,
        secure: true,
        http_only: true,
        max_age,
    }
}

pub(crate) fn cookie_origin() -> Result<crate::config::HttpOrigin, Error> {
    INVOCATION
        .try_with(|invocation| invocation.origin.clone().ok_or(Error::Internal))
        .map_err(|_| Error::Internal)?
}

pub fn log_fields() -> Vec<crate::log::Field> {
    INVOCATION
        .try_with(|invocation| {
            let mut fields = vec![crate::log::Field {
                name: "site".into(),
                value: invocation.site.clone(),
            }];
            if let Some(identity) = &invocation.identity {
                fields.push(crate::log::Field {
                    name: "auth_id".into(),
                    value: identity.id.clone(),
                });
                if let Some(user_id) = identity.user_id {
                    fields.push(crate::log::Field {
                        name: "user_id".into(),
                        value: user_id.to_string(),
                    });
                }
                if let Some(tenant_id) = identity.tenant_id {
                    fields.push(crate::log::Field {
                        name: "tenant_id".into(),
                        value: tenant_id.to_string(),
                    });
                }
            }
            fields
        })
        .unwrap_or_default()
}

fn validate_identity(claims: Option<&Claims>, identity: Option<&Identity>) -> Result<(), Error> {
    match (claims, identity) {
        (None, None) => return Ok(()),
        (Some(_), None) => return Err(Error::Unauthorized),
        (None, Some(_)) => return Err(Error::Internal),
        (Some(claims), Some(identity)) => {
            if identity.id != claims.subject
                || claims.tenant.is_some() != identity.tenant_id.is_some()
                || identity.user_id.is_some_and(|id| id <= 0)
                || identity.tenant_id.is_some_and(|id| id <= 0)
            {
                return Err(Error::Unauthorized);
            }
        }
    }
    validate_identity_text("identity", &identity.expect("checked identity").id)
}

fn validate_request_host(request: &Request, allowed: &[String]) -> Result<(), Error> {
    let host = unique_header(&request.headers, "host")?.ok_or(Error::Unauthorized)?;
    let host = std::str::from_utf8(host).map_err(|_| Error::Unauthorized)?;
    let host = crate::config::normalize_authority_host(host).map_err(|_| Error::Unauthorized)?;
    if !allowed.is_empty() && !allowed.iter().any(|allowed| allowed == &host) {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

fn bearer_token(request: &Request) -> Result<Option<Vec<u8>>, Error> {
    let Some(value) = unique_header(&request.headers, "authorization")? else {
        return Ok(None);
    };
    if !value.is_ascii() || value.len() > MAX_TOKEN_BYTES + 7 {
        return Err(Error::Unauthorized);
    }
    let Some(separator) = value.iter().position(|byte| *byte == b' ') else {
        return Err(Error::Unauthorized);
    };
    let (scheme, token) = (&value[..separator], &value[separator + 1..]);
    if !scheme.eq_ignore_ascii_case(b"bearer")
        || token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token.iter().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(Error::Unauthorized);
    }
    Ok(Some(token.to_vec()))
}

fn cookie_token(request: &Request, name: &str) -> Result<Option<Vec<u8>>, Error> {
    let mut selected = None;
    for header in request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("cookie"))
    {
        let raw = header.value.values();
        if !raw.is_ascii() {
            return Err(Error::Unauthorized);
        }
        for pair in raw.split(|byte| *byte == b';') {
            let pair = pair.trim_ascii();
            let Some(separator) = pair.iter().position(|byte| *byte == b'=') else {
                return Err(Error::Unauthorized);
            };
            if &pair[..separator] != name.as_bytes() {
                continue;
            }
            if selected.is_some() {
                return Err(Error::Unauthorized);
            }
            let encoded = &pair[separator + 1..];
            let encoded = if encoded.len() >= 2
                && encoded.first() == Some(&b'"')
                && encoded.last() == Some(&b'"')
            {
                &encoded[1..encoded.len() - 1]
            } else {
                encoded
            };
            let encoded = encoded.strip_prefix(b"v1.").ok_or(Error::Unauthorized)?;
            if encoded.len() > MAX_TOKEN_BYTES.saturating_mul(2) {
                return Err(Error::Unauthorized);
            }
            let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| Error::Unauthorized)?;
            if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
                return Err(Error::Unauthorized);
            }
            selected = Some(token);
        }
    }
    Ok(selected)
}

fn validate_cookie_origin(
    request: &Request,
    origin: Option<&crate::config::HttpOrigin>,
) -> Result<(), Error> {
    if matches!(request.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        return Ok(());
    }
    let origin = origin.ok_or(Error::Internal)?;
    crate::api::validate_same_origin(&request.headers, origin).map_err(|_| Error::Forbidden)
}

fn unique_header<'a>(headers: &'a [Header], name: &str) -> Result<Option<&'a [u8]>, Error> {
    let mut values = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(name));
    let value = values.next().map(|header| header.value.values());
    if values.next().is_some() {
        return Err(Error::Unauthorized);
    }
    Ok(value)
}

fn verify_jwt(
    token: &[u8],
    site: &str,
    provider_key: &str,
    provider: &AuthProvider,
) -> Result<Claims, Error> {
    let text = std::str::from_utf8(token).map_err(|_| Error::Unauthorized)?;
    let mut parts = text.split('.');
    let header_part = parts.next().ok_or(Error::Unauthorized)?;
    let claims_part = parts.next().ok_or(Error::Unauthorized)?;
    let signature_part = parts.next().ok_or(Error::Unauthorized)?;
    if parts.next().is_some()
        || [header_part, claims_part, signature_part]
            .iter()
            .any(|part| part.is_empty())
    {
        return Err(Error::Unauthorized);
    }
    let header = decode_part(header_part)?;
    let claims = decode_part(claims_part)?;
    let signature = decode_part(signature_part)?;
    let header_text = std::str::from_utf8(&header).map_err(|_| Error::Unauthorized)?;
    let claims_text = std::str::from_utf8(&claims).map_err(|_| Error::Unauthorized)?;
    crate::wire::parse(header_text).map_err(|_| Error::Unauthorized)?;
    crate::wire::parse(claims_text).map_err(|_| Error::Unauthorized)?;
    let header: JwtHeader = serde_json::from_slice(&header).map_err(|_| Error::Unauthorized)?;
    if header.alg != "HS256" || header.typ != "JWT" {
        return Err(Error::Unauthorized);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&provider.jwt_secret().0).map_err(|_| Error::Internal)?;
    mac.update(format!("{header_part}.{claims_part}").as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| Error::Unauthorized)?;

    let claims: JwtClaims = serde_json::from_slice(&claims).map_err(|_| Error::Unauthorized)?;
    let now = unix_seconds()?;
    if claims.iss != provider_key
        || claims.aud != site
        || claims.iat <= 0
        || claims.exp <= now - CLOCK_SKEW_SECONDS
        || claims.iat > now + CLOCK_SKEW_SECONDS
        || claims.nbf.is_some_and(|nbf| nbf > now + CLOCK_SKEW_SECONDS)
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > provider.ttl_seconds() as i64 + CLOCK_SKEW_SECONDS
    {
        return Err(Error::Unauthorized);
    }
    validate_identity_text("subject", &claims.sub)?;
    validate_identity_text("session", &claims.sid)?;
    if let Some(tenant) = &claims.tenant {
        validate_identity_text("tenant", tenant)?;
    }
    Ok(Claims {
        subject: claims.sub,
        session: claims.sid,
        tenant: claims.tenant,
        site: claims.aud,
    })
}

fn encode_jwt(claims: &JwtClaims, provider: &AuthProvider) -> Result<Vec<u8>, Error> {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = serde_json::to_vec(&IssuedHeader {
        alg: "HS256",
        typ: "JWT",
    })
    .map_err(|_| Error::Internal)?;
    let claims = serde_json::to_vec(claims).map_err(|_| Error::Internal)?;
    let signed = format!("{}.{}", engine.encode(header), engine.encode(claims));
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&provider.jwt_secret().0).map_err(|_| Error::Internal)?;
    mac.update(signed.as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(format!("{signed}.{}", engine.encode(signature)).into_bytes())
}

fn decode_part(part: &str) -> Result<Vec<u8>, Error> {
    if part.len() > MAX_JWT_PART_BYTES {
        return Err(Error::Unauthorized);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| Error::Unauthorized)
}

fn unix_seconds() -> Result<i64, Error> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Internal)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| Error::Internal)
}

fn validate_identity_text(_field: &str, value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > MAX_IDENTITY_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        Err(Error::Unauthorized)
    } else {
        Ok(())
    }
}

fn with_invocation<T>(read: impl FnOnce(&Invocation) -> Result<T, Error>) -> Result<T, Error> {
    INVOCATION.try_with(read).map_err(|_| Error::Unauthorized)?
}

fn with_identity<T>(read: impl FnOnce(&Identity) -> Result<T, Error>) -> Result<T, Error> {
    with_invocation(|invocation| {
        invocation
            .identity
            .as_ref()
            .ok_or(Error::Unauthorized)
            .and_then(read)
    })
}
