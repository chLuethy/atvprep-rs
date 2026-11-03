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
