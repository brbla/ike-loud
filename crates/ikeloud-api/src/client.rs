//! Sign-in to iCloud and the Drive calls, over a [`Transport`].
//!
//! The flow was checked against the real service on 2026-11-16
//! (decision 0021):
//!
//! 1. `signin/init` with A, `signin/complete` with M1 and M2 (SRP, [`crate::srp`]);
//!    200 means signed in, 409 means a second factor is needed (`hsa2`).
//! 2. Second factor by SMS: `verify/phone` sends the code,
//!    `verify/phone/securitycode` checks it, `2sv/trust` returns a trust token
//!    that lets the next sign-in skip the SMS.
//! 3. `accountLogin` turns the session token into iCloud cookies and gives the
//!    addresses of the services (`drivews`, `docws`).
//! 4. `validate` checks saved cookies without a password.
//!
//! Responses of the sign-in service are never logged or put into errors in
//! full: they contain the unmasked phone number.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::b64;
use crate::cookies::CookieJar;
use crate::http::{Body, Method, Request, Response, Transport, TransportError, https_host};
use crate::srp::{self, Padding, Protocol, SrpClient, SrpError};

const AUTH: &str = "https://idmsa.apple.com/appleauth/auth";
const SETUP: &str = "https://setup.icloud.com/setup/ws/1";
/// Public key of the icloud.com web app for the sign-in widget.
const WIDGET_KEY: &str = "d39ba9916b7251055b22c7f910e2ea796ee65e98b2ddecea8f5dde8d9d1a815d";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
/// JSON responses are small; anything bigger is not what we asked for.
const JSON_LIMIT: u64 = 16 * 1024 * 1024;

/// Error of an iCloud call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No response (network, TLS, a host outside the allowed list).
    Transport(TransportError),
    /// SRP computation failed.
    Srp(SrpError),
    /// Wrong account name or password.
    BadCredentials,
    /// The server rejected our SRP proof of its own (M2 did not match).
    ServerProofMismatch,
    /// The SMS code was wrong or expired.
    BadCode,
    /// Too many codes sent or tried; wait before the next attempt.
    TooManyCodes,
    /// The account has no trusted phone number for the SMS.
    NoTrustedPhone,
    /// The saved session is no longer valid; sign in again.
    SessionExpired,
    /// iCloud Drive was never opened on this account (402
    /// `CONTAINER_NEEDS_MIGRATION`); it must be set up on icloud.com once.
    DriveNotSetUp,
    /// The account does not offer iCloud Drive.
    DriveUnavailable,
    /// Not signed in to iCloud yet.
    NotSignedIn,
    /// A write found the name taken on the server; nothing was written
    /// (a rename may have landed under another name: see `Client::rename`).
    NameTaken,
    /// A write found another version on the server than it was meant for;
    /// nothing was written.
    VersionChanged,
    /// The server refused a write with a status the client does not know;
    /// nothing was written as far as the server says.
    Rejected {
        /// Which call.
        call: &'static str,
        /// The server's status text.
        status: String,
    },
    /// Unexpected status. The body is not included (it may carry personal data).
    Status {
        /// Which call.
        call: &'static str,
        /// HTTP status.
        status: u16,
    },
    /// The response did not have the expected shape.
    Malformed {
        /// Which call.
        call: &'static str,
        /// What was wrong, without the response content.
        what: String,
    },
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Srp(e) => write!(f, "SRP: {e:?}"),
            Self::BadCredentials => write!(f, "wrong Apple Account or password"),
            Self::ServerProofMismatch => write!(f, "the server did not prove the password"),
            Self::BadCode => write!(f, "wrong or expired code"),
            Self::TooManyCodes => write!(f, "too many codes; try again later"),
            Self::NoTrustedPhone => write!(f, "no trusted phone number"),
            Self::SessionExpired => write!(f, "session expired; sign in again"),
            Self::DriveNotSetUp => {
                write!(f, "iCloud Drive is not set up; open it once on icloud.com")
            }
            Self::DriveUnavailable => write!(f, "iCloud Drive is not available for this account"),
            Self::NotSignedIn => write!(f, "not signed in"),
            Self::NameTaken => write!(f, "the name is taken on iCloud"),
            Self::VersionChanged => write!(f, "the item changed on iCloud"),
            Self::Rejected { call, status } => write!(f, "{call}: refused ({status})"),
            Self::Status { call, status } => write!(f, "{call}: unexpected status {status}"),
            Self::Malformed { call, what } => write!(f, "{call}: unexpected response: {what}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<TransportError> for ApiError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<SrpError> for ApiError {
    fn from(e: SrpError) -> Self {
        Self::Srp(e)
    }
}

/// A trusted phone number for the SMS, as the server shows it (masked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPhone {
    /// Identifier for the code requests.
    pub id: u64,
    /// Masked number, e.g. `••• ••• •37`.
    pub masked: String,
}

/// Result of a password sign-in.
#[derive(Debug)]
pub enum SignIn {
    /// Signed in; iCloud is ready.
    Done,
    /// A code is needed. Send it with [`Client::send_sms`], check it with
    /// [`Client::verify_sms`].
    NeedsCode {
        /// Phones the code can go to.
        phones: Vec<TrustedPhone>,
    },
}

/// Addresses and identity of the signed-in iCloud account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Directory services id, a query parameter of every Drive call.
    pub dsid: String,
    /// Base URL of the Drive web service (listing).
    pub drivews: String,
    /// Base URL of the document service (downloads).
    pub docws: String,
    /// Base URL of the CloudKit database service (changes of the Drive
    /// zone, decision 0025); `None` in sessions saved before it was used.
    #[serde(default)]
    pub ckdatabasews: Option<String>,
}

/// State of a sign-in in progress (between the password and the code).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    session_token: String,
    session_id: Option<String>,
    scnt: Option<String>,
    country: String,
}

/// What survives a restart: cookies, the trust token and the account.
/// Contains secrets; the caller keeps it in the keyring, never in a log.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    account_name: String,
    trust_token: Option<String>,
    cookies: CookieJar,
    account: Option<Account>,
}

impl std::fmt::Debug for SavedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedSession")
            .field("has_trust_token", &self.trust_token.is_some())
            .field("cookies", &self.cookies.len())
            .field("signed_in", &self.account.is_some())
            .finish_non_exhaustive()
    }
}

impl SavedSession {
    /// Serialized form for the keyring.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// From the serialized form.
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    /// Apple Account name the session belongs to.
    pub fn account_name(&self) -> &str {
        &self.account_name
    }
}

/// Client of one Apple Account.
pub struct Client<T: Transport> {
    transport: T,
    state: SavedSession,
    oauth_state: String,
    pending: Option<Pending>,
    padding: Padding,
}

impl<T: Transport> std::fmt::Debug for Client<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct InitResponse {
    iteration: u32,
    salt: String,
    protocol: String,
    b: String,
    c: String,
}

impl<T: Transport> Client<T> {
    /// New client with no session.
    pub fn new(transport: T, account_name: &str) -> Self {
        Self::restore(
            transport,
            SavedSession {
                account_name: account_name.to_string(),
                ..SavedSession::default()
            },
        )
    }

    /// Client from a saved session.
    pub fn restore(transport: T, state: SavedSession) -> Self {
        Self {
            transport,
            state,
            oauth_state: format!("auth-{}", random_hex(16)),
            pending: None,
            padding: Padding::default(),
        }
    }

    /// The transport (to make another client with the same connections).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The session to save (after every successful call that may change it).
    pub fn saved(&self) -> &SavedSession {
        &self.state
    }

    /// The signed-in account, if any.
    pub fn account(&self) -> Option<&Account> {
        self.state.account.as_ref()
    }

    /// Signs in with the password. With a valid trust token from an earlier
    /// SMS, Apple usually skips the code.
    pub fn sign_in(&mut self, password: &str) -> Result<SignIn, ApiError> {
        let srp = SrpClient::new(self.padding)?;
        let init: InitResponse = {
            let body = json!({
                "a": b64::encode(&srp.public_a()),
                "accountName": self.state.account_name,
                "protocols": [Protocol::S2k.name(), Protocol::S2kFo.name()],
            });
            let mut response = self.send(
                Method::Post,
                &format!("{AUTH}/signin/init"),
                self.auth_headers(None),
                Some(body),
            )?;
            match response.status {
                200 => parse_json(&mut response, "signin/init")?,
                400 | 401 | 403 => return Err(ApiError::BadCredentials),
                status => {
                    return Err(ApiError::Status {
                        call: "signin/init",
                        status,
                    });
                }
            }
        };
        let protocol = Protocol::from_name(&init.protocol)?;
        let salt = decode_b64(&init.salt, "signin/init")?;
        let b = decode_b64(&init.b, "signin/init")?;
        let derived = srp::derive_password(password, &salt, init.iteration, protocol)?;
        let proof = srp.process_challenge(&self.state.account_name, &derived, &salt, &b)?;

        let trust: Vec<&str> = self.state.trust_token.as_deref().into_iter().collect();
        let body = json!({
            "accountName": self.state.account_name,
            "rememberMe": true,
            "trustTokens": trust,
            "m1": b64::encode(&proof.m1),
            "m2": b64::encode(&proof.m2),
            "c": init.c,
        });
        let response = self.send(
            Method::Post,
            &format!("{AUTH}/signin/complete?isRememberMeEnabled=true"),
            self.auth_headers(None),
            Some(body),
        )?;
        match response.status {
            200 | 409 => {}
            401 | 403 => return Err(ApiError::BadCredentials),
            412 => {
                return Err(ApiError::Malformed {
                    call: "signin/complete",
                    what: "the account needs an action on appleid.apple.com (412)".to_string(),
                });
            }
            status => {
                return Err(ApiError::Status {
                    call: "signin/complete",
                    status,
                });
            }
        }
        let pending = Pending {
            session_token: required_header(&response, "X-Apple-Session-Token", "signin/complete")?,
            session_id: response.header("X-Apple-ID-Session-Id").map(str::to_string),
            scnt: response.header("scnt").map(str::to_string),
            country: response
                .header("X-Apple-ID-Account-Country")
                .unwrap_or_default()
                .to_string(),
        };
        if response.status == 200 {
            self.account_login(&pending)?;
            return Ok(SignIn::Done);
        }
        self.pending = Some(pending);
        let phones = self.trusted_phones()?;
        Ok(SignIn::NeedsCode { phones })
    }

    fn trusted_phones(&mut self) -> Result<Vec<TrustedPhone>, ApiError> {
        let headers = self.pending_headers()?;
        let mut response = self.send(Method::Get, AUTH, headers, None)?;
        if !matches!(response.status, 200 | 201) {
            return Err(ApiError::Status {
                call: "auth",
                status: response.status,
            });
        }
        let value: Value = parse_json(&mut response, "auth")?;
        self.update_pending(&response);
        Ok(phones_from(&value))
    }

    /// Sends the code by SMS to the phone `phone_id`. Can be repeated.
    pub fn send_sms(&mut self, phone_id: u64) -> Result<(), ApiError> {
        let headers = self.pending_headers()?;
        let body = json!({"phoneNumber": {"id": phone_id}, "mode": "sms"});
        let mut response = self.send(
            Method::Put,
            &format!("{AUTH}/verify/phone"),
            headers,
            Some(body),
        )?;
        self.update_pending(&response);
        match response.status {
            200 => {
                let value: Value = parse_json(&mut response, "verify/phone")?;
                let code = value.get("securityCode");
                let flag = |name: &str| {
                    code.and_then(|c| c.get(name))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                };
                if flag("tooManyCodesSent") || flag("securityCodeLocked") {
                    return Err(ApiError::TooManyCodes);
                }
                Ok(())
            }
            423 => Err(ApiError::TooManyCodes),
            status => Err(ApiError::Status {
                call: "verify/phone",
                status,
            }),
        }
    }

    /// Checks the code, gets the trust token and signs in to iCloud.
    pub fn verify_sms(&mut self, phone_id: u64, code: &str) -> Result<(), ApiError> {
        let headers = self.pending_headers()?;
        let body = json!({
            "securityCode": {"code": code.trim()},
            "phoneNumber": {"id": phone_id},
            "mode": "sms",
        });
        let response = self.send(
            Method::Post,
            &format!("{AUTH}/verify/phone/securitycode"),
            headers,
            Some(body),
        )?;
        self.update_pending(&response);
        match response.status {
            200 | 204 => {}
            400 | 401 => return Err(ApiError::BadCode),
            423 => return Err(ApiError::TooManyCodes),
            status => {
                return Err(ApiError::Status {
                    call: "verify/phone/securitycode",
                    status,
                });
            }
        }
        let headers = self.pending_headers()?;
        let response = self.send(Method::Get, &format!("{AUTH}/2sv/trust"), headers, None)?;
        if !matches!(response.status, 200 | 204) {
            return Err(ApiError::Status {
                call: "2sv/trust",
                status: response.status,
            });
        }
        self.update_pending(&response);
        if let Some(trust) = response.header("X-Apple-TwoSV-Trust-Token") {
            self.state.trust_token = Some(trust.to_string());
        }
        let pending = self.pending.take().ok_or(ApiError::NotSignedIn)?;
        self.account_login(&pending)
    }

    fn account_login(&mut self, pending: &Pending) -> Result<(), ApiError> {
        let body = json!({
            "accountCountryCode": pending.country,
            "dsWebAuthToken": pending.session_token,
            "extended_login": true,
            "trustToken": self.state.trust_token.clone().unwrap_or_default(),
        });
        let mut response = self.send(
            Method::Post,
            &format!("{SETUP}/accountLogin"),
            web_headers(),
            Some(body),
        )?;
        match response.status {
            200 => {}
            401 | 421 => return Err(ApiError::SessionExpired),
            status => {
                return Err(ApiError::Status {
                    call: "accountLogin",
                    status,
                });
            }
        }
        let value: Value = parse_json(&mut response, "accountLogin")?;
        if value.get("hsaChallengeRequired").and_then(Value::as_bool) == Some(true) {
            return Err(ApiError::Malformed {
                call: "accountLogin",
                what: "second factor still required".to_string(),
            });
        }
        self.state.account = Some(account_from(&value, "accountLogin")?);
        self.pending = None;
        Ok(())
    }

    /// Checks the saved cookies without a password and refreshes the service
    /// addresses. [`ApiError::SessionExpired`] means: sign in again.
    pub fn validate(&mut self) -> Result<(), ApiError> {
        let mut response = self.send(
            Method::Post,
            &format!("{SETUP}/validate"),
            web_headers(),
            None,
        )?;
        match response.status {
            200 => {}
            401 | 421 | 450 => {
                self.state.account = None;
                return Err(ApiError::SessionExpired);
            }
            status => {
                return Err(ApiError::Status {
                    call: "validate",
                    status,
                });
            }
        }
        let value: Value = parse_json(&mut response, "validate")?;
        self.state.account = Some(account_from(&value, "validate")?);
        Ok(())
    }

    /// POST with a JSON body to a web service, returning the parsed JSON.
    pub(crate) fn web_json<R: DeserializeOwned>(
        &mut self,
        call: &'static str,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<R, ApiError> {
        let mut response = self.send(method, url, web_headers(), body)?;
        match response.status {
            200 => parse_json(&mut response, call),
            401 | 421 | 450 => Err(ApiError::SessionExpired),
            402 => Err(ApiError::DriveNotSetUp),
            status => Err(ApiError::Status { call, status }),
        }
    }

    /// GET of a body stream (download) with the session cookies.
    pub(crate) fn web_stream(&mut self, url: &str) -> Result<Response, ApiError> {
        let response = self.send(Method::Get, url, web_headers(), None)?;
        match response.status {
            200 => Ok(response),
            401 | 421 | 450 => Err(ApiError::SessionExpired),
            status => Err(ApiError::Status {
                call: "download",
                status,
            }),
        }
    }

    fn send(
        &mut self,
        method: Method,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Value>,
    ) -> Result<Response, ApiError> {
        let body = match body {
            Some(b) => Body::Bytes(b.to_string().into_bytes()),
            None => Body::Empty,
        };
        self.send_body(method, url, headers, body)
    }

    /// Sends any body, with the session cookies for the host.
    pub(crate) fn send_body(
        &mut self,
        method: Method,
        url: &str,
        mut headers: Vec<(String, String)>,
        body: Body<'_>,
    ) -> Result<Response, ApiError> {
        let host = https_host(url)
            .ok_or_else(|| TransportError::HostNotAllowed(url.to_string()))?
            .to_string();
        let path = url
            .strip_prefix("https://")
            .and_then(|rest| rest.find('/').and_then(|i| rest.get(i..)))
            .unwrap_or("/");
        if let Some(cookie) = self.state.cookies.header(&host, path) {
            headers.push(("Cookie".to_string(), cookie));
        }
        let response = self.transport.send(Request {
            method,
            url: url.to_string(),
            headers,
            body,
        })?;
        self.state
            .cookies
            .store(&host, response.headers_named("Set-Cookie"));
        Ok(response)
    }

    fn auth_headers(&self, pending: Option<&Pending>) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = [
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("User-Agent", USER_AGENT),
            ("Origin", "https://idmsa.apple.com"),
            ("Referer", "https://idmsa.apple.com/"),
            ("X-Apple-Widget-Key", WIDGET_KEY),
            ("X-Apple-OAuth-Client-Id", WIDGET_KEY),
            ("X-Apple-OAuth-Client-Type", "firstPartyAuth"),
            ("X-Apple-OAuth-Redirect-URI", "https://www.icloud.com"),
            ("X-Apple-OAuth-Require-Grant-Code", "true"),
            ("X-Apple-OAuth-Response-Mode", "web_message"),
            ("X-Apple-OAuth-Response-Type", "code"),
            ("X-Apple-OAuth-State", self.oauth_state.as_str()),
            ("X-Apple-Frame-Id", self.oauth_state.as_str()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        if let Some(p) = pending {
            headers.push(("X-Apple-Session-Token".to_string(), p.session_token.clone()));
            if let Some(id) = &p.session_id {
                headers.push(("X-Apple-ID-Session-Id".to_string(), id.clone()));
            }
            if let Some(scnt) = &p.scnt {
                headers.push(("scnt".to_string(), scnt.clone()));
            }
        }
        headers
    }

    fn pending_headers(&self) -> Result<Vec<(String, String)>, ApiError> {
        let pending = self.pending.as_ref().ok_or(ApiError::NotSignedIn)?;
        Ok(self.auth_headers(Some(pending)))
    }

    /// The sign-in service rotates these headers; the next call must send the new ones.
    fn update_pending(&mut self, response: &Response) {
        if let Some(p) = self.pending.as_mut() {
            if let Some(v) = response.header("X-Apple-Session-Token") {
                p.session_token = v.to_string();
            }
            if let Some(v) = response.header("X-Apple-ID-Session-Id") {
                p.session_id = Some(v.to_string());
            }
            if let Some(v) = response.header("scnt") {
                p.scnt = Some(v.to_string());
            }
        }
    }
}

pub(crate) fn web_headers() -> Vec<(String, String)> {
    [
        ("Content-Type", "application/json"),
        ("Accept", "application/json"),
        ("User-Agent", USER_AGENT),
        ("Origin", "https://www.icloud.com"),
        ("Referer", "https://www.icloud.com/"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

pub(crate) fn parse_json<R: DeserializeOwned>(
    response: &mut Response,
    call: &'static str,
) -> Result<R, ApiError> {
    let body = response.read_body(JSON_LIMIT)?;
    serde_json::from_slice(&body).map_err(|e| ApiError::Malformed {
        call,
        // Line and column only; the message of serde_json can quote the input.
        what: format!("invalid JSON at line {} column {}", e.line(), e.column()),
    })
}

fn decode_b64(text: &str, call: &'static str) -> Result<Vec<u8>, ApiError> {
    b64::decode(text).ok_or_else(|| ApiError::Malformed {
        call,
        what: "invalid base64".to_string(),
    })
}

fn required_header(
    response: &Response,
    name: &str,
    call: &'static str,
) -> Result<String, ApiError> {
    response
        .header(name)
        .map(str::to_string)
        .ok_or_else(|| ApiError::Malformed {
            call,
            what: format!("missing header {name}"),
        })
}

fn phones_from(value: &Value) -> Vec<TrustedPhone> {
    value
        .get("trustedPhoneNumbers")
        .and_then(Value::as_array)
        .map(|phones| {
            phones
                .iter()
                .filter_map(|p| {
                    Some(TrustedPhone {
                        id: p.get("id")?.as_u64()?,
                        masked: p
                            .get("obfuscatedNumber")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn account_from(value: &Value, call: &'static str) -> Result<Account, ApiError> {
    let malformed = |what: &str| ApiError::Malformed {
        call,
        what: what.to_string(),
    };
    let dsid = match value.get("dsInfo").and_then(|d| d.get("dsid")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(malformed("missing dsInfo.dsid")),
    };
    let service = |name: &str| {
        value
            .get("webservices")
            .and_then(|w| w.get(name))
            .and_then(|s| s.get("url"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match (service("drivews"), service("docws")) {
        (Some(drivews), Some(docws)) => Ok(Account {
            dsid,
            drivews,
            docws,
            ckdatabasews: service("ckdatabasews"),
        }),
        _ => Err(ApiError::DriveUnavailable),
    }
}

pub(crate) fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    // A missing random source only makes the OAuth state guessable, and it
    // is no secret: it ties the requests of one sign-in together.
    let _ = getrandom::getrandom(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
