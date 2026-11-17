//! Cookies of the iCloud web session.
//!
//! A deliberately small jar: name, value, domain and path from `Set-Cookie`,
//! sent back to hosts in that domain. Expiry is left to the server (an
//! expired session answers 421 and the client signs in again); a cookie
//! with `Max-Age=0` or a past `Expires` sent to delete it is removed.

use serde::{Deserialize, Serialize};

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookie {
    /// Name.
    pub name: String,
    /// Value.
    pub value: String,
    /// Domain without a leading dot; also matches its subdomains.
    pub domain: String,
    /// Path prefix.
    pub path: String,
}

/// The jar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    /// Stores the cookies of one response from `host`.
    pub fn store<'a>(&mut self, host: &str, set_cookies: impl IntoIterator<Item = &'a str>) {
        for header in set_cookies {
            let Some((cookie, delete)) = parse(host, header) else {
                continue;
            };
            self.cookies.retain(|c| {
                !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
            });
            if !delete {
                self.cookies.push(cookie);
            }
        }
    }

    /// The `Cookie` header for a request to `host` and `path`, if any applies.
    pub fn header(&self, host: &str, path: &str) -> Option<String> {
        let host = host.to_ascii_lowercase();
        let parts: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| domain_matches(&host, &c.domain) && path.starts_with(&c.path))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    /// Whether a cookie of this name is stored.
    pub fn contains(&self, name: &str) -> bool {
        self.cookies.iter().any(|c| c.name == name)
    }

    /// Number of stored cookies.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Whether the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// Parses one `Set-Cookie`. Returns the cookie and whether it asks to be deleted.
fn parse(host: &str, header: &str) -> Option<(Cookie, bool)> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.trim().split_once('=')?;
    if name.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let mut domain = host.clone();
    let mut path = "/".to_string();
    let mut delete = false;
    for attribute in parts {
        let (key, val) = attribute
            .trim()
            .split_once('=')
            .unwrap_or((attribute.trim(), ""));
        match key.to_ascii_lowercase().as_str() {
            "domain" => {
                let d = val.trim().trim_start_matches('.').to_ascii_lowercase();
                // A server may only set cookies for its own domain or a parent of it.
                if !domain_matches(&host, &d) || !d.contains('.') {
                    return None;
                }
                domain = d;
            }
            "path" if val.starts_with('/') => path = val.to_string(),
            "max-age" => delete |= val.trim().starts_with('-') || val.trim() == "0",
            "expires" => delete |= expired(val),
            _ => {}
        }
    }
    Some((
        Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain,
            path,
        },
        delete,
    ))
}

/// Apple deletes cookies with `Expires=Thu, 01-Jan-1970 …`; a year before
/// 2000 is taken as a deletion, other dates are left to the server.
fn expired(value: &str) -> bool {
    value
        .split([' ', '-'])
        .filter_map(|part| part.parse::<u32>().ok())
        .any(|n| (1000..2000).contains(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_sends_by_domain() {
        let mut jar = CookieJar::default();
        jar.store(
            "setup.icloud.com",
            [
                "X-APPLE-WEBAUTH-TOKEN=\"v=2:t=AA\";Path=/;Domain=.icloud.com;Secure;HttpOnly",
                "local=1; Path=/setup",
            ],
        );
        assert_eq!(
            jar.header("p63-drivews.icloud.com", "/retrieve"),
            Some("X-APPLE-WEBAUTH-TOKEN=\"v=2:t=AA\"".to_string())
        );
        assert_eq!(
            jar.header("setup.icloud.com", "/setup/ws/1/validate"),
            Some("X-APPLE-WEBAUTH-TOKEN=\"v=2:t=AA\"; local=1".to_string())
        );
        assert_eq!(jar.header("idmsa.apple.com", "/"), None);
        assert_eq!(jar.header("evilicloud.com", "/"), None);
    }

    #[test]
    fn foreign_domain_is_refused() {
        let mut jar = CookieJar::default();
        jar.store(
            "setup.icloud.com",
            [
                "a=1; Domain=apple.com",
                "b=2; Domain=com",
                "c=3; Domain=www.setup.icloud.com",
            ],
        );
        assert!(jar.is_empty());
    }

    #[test]
    fn replaced_and_deleted() {
        let mut jar = CookieJar::default();
        jar.store("idmsa.apple.com", ["aasp=1; Domain=.idmsa.apple.com"]);
        jar.store("idmsa.apple.com", ["aasp=2; Domain=.idmsa.apple.com"]);
        assert_eq!(
            jar.header("idmsa.apple.com", "/"),
            Some("aasp=2".to_string())
        );
        jar.store(
            "idmsa.apple.com",
            ["aasp=; Domain=.idmsa.apple.com; Expires=Thu, 01-Jan-1970 00:00:10 GMT"],
        );
        assert!(jar.is_empty());
        jar.store("idmsa.apple.com", ["x=1", "x=; Max-Age=0"]);
        assert!(jar.is_empty());
    }

    #[test]
    fn malformed_is_skipped() {
        let mut jar = CookieJar::default();
        jar.store("setup.icloud.com", ["novalue", "=x", ""]);
        assert!(jar.is_empty());
    }
}
