mod session;

use poem::Request;
use poem::Response;
use poem::web::cookie::{Cookie, SameSite};
use rand::distr::Alphanumeric;
use rand::{Rng, rng};

use crate::database::DatabaseError;

pub use session::{CredentialKind, CurrentCredential, CurrentUser, RequireSession, SessionUser};

pub const SESSION_COOKIE: &str = "__Host-error-menu-session";
pub const SESSION_SECONDS: u64 = 60 * 60 * 24 * 14;

pub fn log_database_error(operation: &'static str, error: &DatabaseError) {
    match error.database_code() {
        Some(database_code) => tracing::error!(
            operation,
            error_kind = error.kind(),
            database_code,
            "authentication storage operation failed"
        ),
        None => tracing::error!(
            operation,
            error_kind = error.kind(),
            "authentication storage operation failed"
        ),
    }
}

pub fn random_token() -> String {
    rng()
        .sample_iter(Alphanumeric)
        .take(64)
        .map(char::from)
        .collect()
}

/// Every cookie this application sets is scoped the same way, and the `__Host-` prefix
/// only holds while the path, secure and domain rules stay as they are here.
pub struct ScopedCookie(pub &'static str);

impl ScopedCookie {
    pub fn read(&self, request: &Request) -> Option<String> {
        request
            .headers()
            .get_all("cookie")
            .iter()
            .find_map(|value| {
                value.to_str().ok().and_then(|header| {
                    header.split(';').map(str::trim).find_map(|item| {
                        Cookie::parse(item)
                            .ok()
                            .filter(|cookie| cookie.name() == self.0)
                            .map(|cookie| cookie.value_str().to_owned())
                    })
                })
            })
    }

    pub fn set(&self, value: &str) -> Cookie {
        let mut cookie = Cookie::new_with_str(self.0, value);
        cookie.set_http_only(true);
        cookie.set_secure(true);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_path("/");
        cookie
    }

    pub fn clear(&self, mut response: Response) -> Response {
        let mut cookie = self.set("");
        cookie.make_removal();
        response.headers_mut().append(
            "set-cookie",
            cookie.to_string().parse().expect("valid cookie"),
        );
        response
    }
}
