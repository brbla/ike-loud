//! Sign-in and Drive calls against a scripted fake server.
//!
//! The fake answers in the shapes recorded from the real service on
//! 2026-11-16 (decision 0021) and records every request, so the tests check
//! both what the client does with an answer and what it sends.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)] // test code

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::rc::Rc;

use ikeloud_api::http::{
    Method, Request, Response, Transport, TransportError, host_allowed, https_host,
};
use ikeloud_api::{ApiError, Client, Kind, ROOT, SavedSession, SignIn};

type Reply = (u16, Vec<(&'static str, &'static str)>, String);

#[derive(Clone, Default)]
struct Fake {
    replies: Rc<RefCell<VecDeque<Reply>>>,
    sent: Rc<RefCell<Vec<Request>>>,
}

impl Fake {
    fn reply(&self, status: u16, headers: &[(&'static str, &'static str)], body: &str) -> &Self {
        self.replies
            .borrow_mut()
            .push_back((status, headers.to_vec(), body.to_string()));
        self
    }

    fn sent(&self) -> Vec<Request> {
        self.sent.borrow().clone()
    }

    fn body(&self, index: usize) -> serde_json::Value {
        let sent = self.sent();
        let body = sent[index].body.clone().unwrap_or_default();
        serde_json::from_slice(&body).unwrap()
    }

    fn header(&self, index: usize, name: &str) -> Option<String> {
        self.sent()[index]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

impl Transport for Fake {
    fn send(&self, request: Request) -> Result<Response, TransportError> {
        self.sent.borrow_mut().push(request);
        let (status, headers, body) = self
            .replies
            .borrow_mut()
            .pop_front()
            .expect("the test scripted fewer replies than the client sent requests");
        Ok(Response {
            status,
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: Box::new(Cursor::new(body.into_bytes())),
        })
    }
}

const ACCOUNT: &str = "someone@example.com";

/// A valid SRP challenge: any salt and a B in range.
const INIT: &str = r#"{"iteration": 1000, "salt": "qdtMfzwVrpg7OTRSXr3ZaQ==", "protocol": "s2k", "b": "AgM=", "c": "c-token"}"#;

const COMPLETE_HEADERS: &[(&str, &str)] = &[
    ("X-Apple-Session-Token", "session-1"),
    ("X-Apple-ID-Session-Id", "sid-1"),
    ("scnt", "scnt-1"),
    ("X-Apple-ID-Account-Country", "CZE"),
    (
        "Set-Cookie",
        "aasp=A1; Domain=idmsa.apple.com; Path=/; Secure",
    ),
];

const ACCOUNT_LOGIN: &str = r#"{
  "dsInfo": {"dsid": "123456", "isWebAccessAllowed": true},
  "hsaChallengeRequired": false,
  "webservices": {
    "drivews": {"url": "https://p63-drivews.icloud.com:443", "status": "active"},
    "docws": {"url": "https://p63-docws.icloud.com:443", "status": "active"}
  }
}"#;

const LOGIN_COOKIES: &[(&str, &str)] = &[(
    "Set-Cookie",
    "X-APPLE-WEBAUTH-TOKEN=\"v=2:t=T\"; Domain=.icloud.com; Path=/; Secure; HttpOnly",
)];

const PHONES: &str = r#"{"trustedPhoneNumbers": [{"id": 1, "obfuscatedNumber": "••• ••• •37", "numberWithDialCode": "+420 777 000 037", "pushMode": "sms"}]}"#;

fn signed_in() -> (Fake, Client<Fake>) {
    let fake = Fake::default();
    fake.reply(200, &[], INIT)
        .reply(200, COMPLETE_HEADERS, "{}")
        .reply(200, LOGIN_COOKIES, ACCOUNT_LOGIN);
    let mut client = Client::new(fake.clone(), ACCOUNT);
    assert!(matches!(client.sign_in("secret").unwrap(), SignIn::Done));
    (fake, client)
}

#[test]
fn sign_in_without_code() {
    let (fake, client) = signed_in();
    let sent = fake.sent();
    assert_eq!(sent.len(), 3);
    assert!(sent[0].url.ends_with("/appleauth/auth/signin/init"));
    let init = fake.body(0);
    assert_eq!(init["accountName"], ACCOUNT);
    assert_eq!(init["protocols"], serde_json::json!(["s2k", "s2k_fo"]));
    // A is the 2048-bit group number in base64 (256 bytes, padded).
    assert_eq!(init["a"].as_str().unwrap().len(), 344);

    let complete = fake.body(1);
    assert_eq!(complete["c"], "c-token");
    assert_eq!(complete["trustTokens"], serde_json::json!([]));
    assert_eq!(complete["m1"].as_str().unwrap().len(), 44);
    // The cookie from the sign-in service goes back to it, not elsewhere.
    assert_eq!(fake.header(1, "Cookie"), None);
    assert_eq!(fake.header(2, "Cookie"), None);

    assert!(sent[2].url.ends_with("/setup/ws/1/accountLogin"));
    let login = fake.body(2);
    assert_eq!(login["dsWebAuthToken"], "session-1");
    assert_eq!(login["accountCountryCode"], "CZE");
    assert_eq!(login["extended_login"], true);

    let account = client.account().unwrap();
    assert_eq!(account.dsid, "123456");
    assert_eq!(account.drivews, "https://p63-drivews.icloud.com:443");
}

#[test]
fn sign_in_with_sms() {
    let fake = Fake::default();
    fake.reply(200, &[], INIT)
        .reply(409, COMPLETE_HEADERS, r#"{"authType": "hsa2"}"#)
        .reply(201, &[("scnt", "scnt-2")], PHONES)
        .reply(
            200,
            &[("scnt", "scnt-3")],
            r#"{"securityCode": {"tooManyCodesSent": false}}"#,
        )
        .reply(400, &[], r#"{"service_errors": []}"#)
        .reply(200, &[("scnt", "scnt-4")], "{}")
        .reply(
            204,
            &[
                ("X-Apple-TwoSV-Trust-Token", "trust-1"),
                ("X-Apple-Session-Token", "session-2"),
            ],
            "",
        )
        .reply(200, LOGIN_COOKIES, ACCOUNT_LOGIN);
    let mut client = Client::new(fake.clone(), ACCOUNT);
    let SignIn::NeedsCode { phones } = client.sign_in("secret").unwrap() else {
        panic!("expected a code request");
    };
    assert_eq!(phones.len(), 1);
    assert_eq!(phones[0].id, 1);
    // Only the masked number reaches the caller.
    assert_eq!(phones[0].masked, "••• ••• •37");
    assert!(!format!("{phones:?}").contains("777"));

    client.send_sms(1).unwrap();
    assert_eq!(client.verify_sms(1, "000000"), Err(ApiError::BadCode));
    client.verify_sms(1, " 241455\n").unwrap();

    let sent = fake.sent();
    assert_eq!(sent[2].method, Method::Get);
    assert_eq!(
        fake.header(2, "X-Apple-Session-Token").as_deref(),
        Some("session-1")
    );
    assert_eq!(fake.header(2, "scnt").as_deref(), Some("scnt-1"));
    assert_eq!(sent[3].method, Method::Put);
    assert!(sent[3].url.ends_with("/verify/phone"));
    // Rotated headers are sent on the next call.
    assert_eq!(fake.header(3, "scnt").as_deref(), Some("scnt-2"));
    assert_eq!(fake.header(4, "scnt").as_deref(), Some("scnt-3"));
    assert_eq!(fake.body(5)["securityCode"]["code"], "241455");
    assert!(sent[6].url.ends_with("/2sv/trust"));
    assert_eq!(fake.body(7)["dsWebAuthToken"], "session-2");
    assert_eq!(fake.body(7)["trustToken"], "trust-1");
    assert!(client.account().is_some());

    // The trust token goes with the next sign-in, so Apple can skip the SMS.
    let saved = SavedSession::from_json(&client.saved().to_json()).unwrap();
    let again = Fake::default();
    again
        .reply(200, &[], INIT)
        .reply(200, COMPLETE_HEADERS, "{}")
        .reply(200, LOGIN_COOKIES, ACCOUNT_LOGIN);
    let mut client = Client::restore(again.clone(), saved);
    assert!(matches!(client.sign_in("secret").unwrap(), SignIn::Done));
    assert_eq!(again.body(1)["trustTokens"], serde_json::json!(["trust-1"]));
}

#[test]
fn too_many_codes() {
    let fake = Fake::default();
    fake.reply(200, &[], INIT)
        .reply(409, COMPLETE_HEADERS, "{}")
        .reply(200, &[], PHONES)
        .reply(200, &[], r#"{"securityCode": {"tooManyCodesSent": true}}"#)
        .reply(423, &[], "{}");
    let mut client = Client::new(fake, ACCOUNT);
    client.sign_in("secret").unwrap();
    assert_eq!(client.send_sms(1), Err(ApiError::TooManyCodes));
    assert_eq!(client.send_sms(1), Err(ApiError::TooManyCodes));
}

#[test]
fn wrong_password() {
    let fake = Fake::default();
    fake.reply(200, &[], INIT)
        .reply(401, &[], r#"{"serviceErrors": []}"#);
    let mut client = Client::new(fake, ACCOUNT);
    assert_eq!(
        client.sign_in("wrong").unwrap_err(),
        ApiError::BadCredentials
    );
    assert!(client.account().is_none());
}

#[test]
fn code_calls_need_a_pending_sign_in() {
    let mut client = Client::new(Fake::default(), ACCOUNT);
    assert_eq!(client.send_sms(1), Err(ApiError::NotSignedIn));
    assert_eq!(client.verify_sms(1, "1"), Err(ApiError::NotSignedIn));
}

#[test]
fn malformed_challenge() {
    for body in [
        r#"{"iteration": 1, "salt": "!!", "protocol": "s2k", "b": "AgM=", "c": "c"}"#,
        r#"{"iteration": 1, "salt": "AA==", "protocol": "s3k", "b": "AgM=", "c": "c"}"#,
        r#"{"iteration": 1, "salt": "AA==", "protocol": "s2k", "b": "AA==", "c": "c"}"#,
        r#"not json"#,
    ] {
        let fake = Fake::default();
        fake.reply(200, &[], body);
        let mut client = Client::new(fake, ACCOUNT);
        let err = client.sign_in("secret").unwrap_err();
        assert!(
            matches!(err, ApiError::Malformed { .. } | ApiError::Srp(_)),
            "{body}: {err:?}"
        );
    }
}

#[test]
fn validate_and_expiry() {
    let (fake, mut client) = signed_in();
    fake.reply(200, &[], ACCOUNT_LOGIN).reply(421, &[], "{}");
    client.validate().unwrap();
    // The iCloud cookie goes to every *.icloud.com service.
    assert_eq!(
        fake.header(3, "Cookie").as_deref(),
        Some("X-APPLE-WEBAUTH-TOKEN=\"v=2:t=T\"")
    );
    assert_eq!(client.validate(), Err(ApiError::SessionExpired));
    assert!(client.account().is_none());
}

#[test]
fn account_without_drive() {
    let fake = Fake::default();
    fake.reply(200, &[], INIT)
        .reply(200, COMPLETE_HEADERS, "{}")
        .reply(200, &[], r#"{"dsInfo": {"dsid": 1}, "webservices": {}}"#);
    let mut client = Client::new(fake, ACCOUNT);
    assert_eq!(
        client.sign_in("secret").unwrap_err(),
        ApiError::DriveUnavailable
    );
}

const ROOT_LISTING: &str = r#"[{
  "drivewsid": "FOLDER::com.apple.CloudDocs::root", "docwsid": "root", "name": "", "etag": "e", "type": "FOLDER",
  "items": [
    {"drivewsid": "FILE::com.apple.CloudDocs::DE61", "docwsid": "DE61", "name": "1494409616350-11", "extension": "jpg",
     "size": 11, "etag": "9::8", "type": "FILE", "parentId": "FOLDER::com.apple.CloudDocs::root",
     "dateModified": "2025-03-20T22:39:45Z", "zone": "com.apple.CloudDocs"},
    {"drivewsid": "FOLDER::com.apple.CloudDocs::7A6D", "docwsid": "7A6D", "name": "Test", "etag": "d", "type": "FOLDER",
     "parentId": "FOLDER::com.apple.CloudDocs::root"},
    {"drivewsid": "FOLDER::com.apple.Pages::documents", "docwsid": "documents", "name": "Pages", "etag": "1", "type": "APP_LIBRARY"},
    {"drivewsid": "FILE::com.apple.CloudDocs::0001", "docwsid": "0001", "name": "Makefile", "extension": "", "size": 0, "etag": "2", "type": "FILE"}
  ]
}]"#;

#[test]
fn list_root() {
    let (fake, mut client) = signed_in();
    fake.reply(200, &[], ROOT_LISTING);
    let folder = client.list_folder(ROOT).unwrap();
    let sent = fake.sent();
    assert_eq!(
        sent[3].url,
        "https://p63-drivews.icloud.com:443/retrieveItemDetailsInFolders?dsid=123456"
    );
    assert_eq!(fake.body(3)[0]["drivewsid"], ROOT);
    assert_eq!(folder.etag, "e");
    let names: Vec<(&str, Kind)> = folder
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("1494409616350-11.jpg", Kind::File),
            ("Test", Kind::Folder),
            ("Pages", Kind::Other),
            ("Makefile", Kind::File)
        ]
    );
    let file = &folder.items[0];
    assert_eq!(file.docwsid, "DE61");
    assert_eq!(file.size, Some(11));
    assert_eq!(file.etag, "9::8");
    assert_eq!(file.parent.as_deref(), Some(ROOT));
    assert_eq!(file.modified.as_deref(), Some("2025-03-20T22:39:45Z"));
}

#[test]
fn drive_not_set_up() {
    let (fake, mut client) = signed_in();
    fake.reply(
        402,
        &[],
        r#"{"errorReason": "CONTAINER_NEEDS_MIGRATION", "errorCode": 402}"#,
    );
    assert_eq!(client.list_folder(ROOT), Err(ApiError::DriveNotSetUp));
}

#[test]
fn listing_without_the_folder() {
    let (fake, mut client) = signed_in();
    fake.reply(200, &[], "[]");
    assert!(matches!(
        client.list_folder(ROOT),
        Err(ApiError::Malformed { .. })
    ));
}

#[test]
fn not_signed_in() {
    let mut client = Client::new(Fake::default(), ACCOUNT);
    assert_eq!(client.list_folder(ROOT), Err(ApiError::NotSignedIn));
}

#[test]
fn download() {
    let (fake, mut client) = signed_in();
    fake.reply(200, &[], ROOT_LISTING);
    let item = client.list_folder(ROOT).unwrap().items.remove(0);

    fake.reply(
        200,
        &[],
        r#"{"data_token": {"url": "https://cvws.icloud-content.com/B/abc", "token": "t"}}"#,
    )
    .reply(200, &[], "hello world");
    let mut out = Vec::new();
    assert_eq!(client.download(&item, &mut out).unwrap(), 11);
    assert_eq!(out, b"hello world");
    let sent = fake.sent();
    assert_eq!(
        sent[4].url,
        "https://p63-docws.icloud.com:443/ws/com.apple.CloudDocs/download/by_id?document_id=DE61&dsid=123456"
    );
    assert_eq!(sent[5].url, "https://cvws.icloud-content.com/B/abc");

    // A short body is an error, not a silently shorter file.
    fake.reply(
        200,
        &[],
        r#"{"data_token": {"url": "https://cvws.icloud-content.com/B/abc"}}"#,
    )
    .reply(200, &[], "hello");
    let mut out = Vec::new();
    assert!(matches!(
        client.download(&item, &mut out),
        Err(ApiError::Malformed { .. })
    ));

    fake.reply(200, &[], "{}");
    assert!(matches!(
        client.download(&item, &mut Vec::new()),
        Err(ApiError::Malformed { .. })
    ));
}

#[test]
fn allowed_hosts() {
    for host in [
        "idmsa.apple.com",
        "setup.icloud.com",
        "p63-drivews.icloud.com",
        "cvws.icloud-content.com",
        "SETUP.ICLOUD.COM",
    ] {
        assert!(host_allowed(host), "{host}");
    }
    for host in [
        "apple.com",
        "icloud.com",
        "evilicloud.com",
        "icloud.com.evil.net",
        "gsa.apple.com",
        ".icloud.com",
    ] {
        assert!(!host_allowed(host), "{host}");
    }
    assert_eq!(
        https_host("https://p63-docws.icloud.com:443/ws?x=1"),
        Some("p63-docws.icloud.com")
    );
    assert_eq!(
        https_host("https://setup.icloud.com"),
        Some("setup.icloud.com")
    );
    assert_eq!(https_host("http://setup.icloud.com/"), None);
    assert_eq!(https_host("https://user@evil.com/"), None);
    assert_eq!(https_host("https:///x"), None);
}

#[test]
fn real_transport_refuses_other_hosts() {
    use ikeloud_api::HttpsTransport;
    let transport = HttpsTransport::new();
    for url in [
        "https://example.com/",
        "http://setup.icloud.com/",
        "https://x@setup.icloud.com/",
    ] {
        let request = Request {
            method: Method::Get,
            url: url.to_string(),
            headers: vec![],
            body: None,
        };
        assert!(
            matches!(
                transport.send(request),
                Err(TransportError::HostNotAllowed(_))
            ),
            "{url}"
        );
    }
}

#[test]
fn saved_session_hides_secrets() {
    let (_, client) = signed_in();
    let debug = format!("{:?} {:?}", client.saved(), client);
    assert!(!debug.contains("v=2:t=T"), "{debug}");
    assert!(!debug.contains("session-1"), "{debug}");
    let json = client.saved().to_json();
    let restored = SavedSession::from_json(&json).unwrap();
    assert_eq!(&restored, client.saved());
    assert_eq!(restored.account_name(), ACCOUNT);
    assert_eq!(SavedSession::from_json("{"), None);
}
