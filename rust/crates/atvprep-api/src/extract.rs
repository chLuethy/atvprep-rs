//! Authentication for routes that require a browser session.

use crate::{routes_auth, state::AppState};
use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::Response,
};

/// A verified session, identified only by its internal database ID.
pub struct AuthenticatedSession {
    pub id: i64,
}

impl FromRequestParts<AppState> for AuthenticatedSession {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let path = parts.uri.path().to_owned();
        let token = routes_auth::cookie(&parts.headers, state.config.session_cookie_secure)
            .ok_or_else(|| routes_auth::failure(StatusCode::UNAUTHORIZED, &path))?;
        let state_for_auth = state.clone();
        let session = tokio::task::spawn_blocking(move || {
            let conn = state_for_auth.db.conn().map_err(|_| ())?;
            routes_auth::service(&state_for_auth, &conn)?
                .authenticate(&token)
                .map_err(|_| ())
        })
        .await;
        let Ok(Ok(id)) = session else {
            return Err(routes_auth::failure(StatusCode::UNAUTHORIZED, &path));
        };
        if !routes_auth::allowed(&parts.headers, state) {
            return Err(routes_auth::failure(StatusCode::FORBIDDEN, &path));
        }
        Ok(Self { id })
    }
}

#[cfg(test)]
mod http_tests {
    use crate::routes_auth;
    use crate::test_support::state;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tower::ServiceExt;

    #[tokio::test]
    async fn authenticated_extractor_checks_session_and_origin() {
        let state = state();
        let mut conn = state.db.conn().unwrap();
        atvprep_manager::schema::migrate(&mut conn).unwrap();
        let token = routes_auth::service(&state, &conn)
            .unwrap()
            .create()
            .unwrap()
            .token;
        let cookie = format!("__Host-atvprep_session={token}");
        let app = Router::new()
            .route(
                "/protected",
                get(|_: crate::extract::AuthenticatedSession| async { StatusCode::OK }),
            )
            .with_state(state);
        let request = |origin: Option<&str>, cookie: Option<&str>| {
            let mut request = axum::http::Request::get("/protected");
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            if let Some(cookie) = cookie {
                request = request.header("cookie", cookie);
            }
            request.body(axum::body::Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request(Some("http://localhost"), None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Some("http://evil"), Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(None, Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.oneshot(request(Some("http://localhost"), Some(&cookie)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}
