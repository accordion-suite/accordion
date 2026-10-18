//! API errors, returned as JSON: `{"error": "<message for the user>"}`.

use accordion_core::identity;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn not_signed_in() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "You're not signed in, or your session has ended. Sign in, then try again.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl From<identity::Error> for ApiError {
    fn from(err: identity::Error) -> Self {
        use identity::Error::*;
        let status = match err {
            InvalidUsername | InvalidDisplayName | PasswordTooShort | PasswordTooLong => {
                StatusCode::BAD_REQUEST
            }
            UsernameTaken | OwnerExists => StatusCode::CONFLICT,
            UserNotFound => StatusCode::NOT_FOUND,
            WrongCredentials => StatusCode::UNAUTHORIZED,
            Database(_) | Hash(_) => {
                // The details are for the admin's log, not the user's screen.
                tracing::error!("identity error: {err}");
                return Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Accordion ran into a problem on the server. Try again in a moment. \
                     If it keeps happening, ask your admin to check the server log.",
                );
            }
        };
        Self::new(status, err.to_string())
    }
}
