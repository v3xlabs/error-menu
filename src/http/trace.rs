use std::time::Instant;

use poem::http::HeaderValue;
use poem::{Endpoint, IntoResponse, Middleware, Request, Response};
use tracing::Instrument;

use crate::trace;

pub const TRACE_HEADER: &str = "x-trace-id";

/// Puts one identifier on every request and on everything it logs, and gives it back in
/// the response, so a reader reporting a failure hands over the string that finds it.
pub struct RequestTrace;

pub struct RequestTraceEndpoint<E> {
    endpoint: E,
}

impl<E> Middleware<E> for RequestTrace
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = RequestTraceEndpoint<E>;

    fn transform(&self, endpoint: E) -> Self::Output {
        RequestTraceEndpoint { endpoint }
    }
}

impl<E> Endpoint for RequestTraceEndpoint<E>
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, request: Request) -> poem::Result<Self::Output> {
        let id = request
            .headers()
            .get(TRACE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(trace::accepted)
            .map_or_else(trace::new_id, str::to_owned);
        let span = tracing::info_span!(
            "request",
            trace = %id,
            method = %request.method(),
            path = request.uri().path(),
            remote = %request.remote_addr(),
        );

        async {
            let started = Instant::now();
            // A rejected extractor and a handler's own response both end up as one status
            // here, so the log line is the same shape either way.
            let mut response = match self.endpoint.call(request).await {
                Ok(response) => response.into_response(),
                Err(error) => error.into_response(),
            };
            tracing::info!(
                status = response.status().as_u16(),
                milliseconds = started.elapsed().as_millis(),
                "request"
            );
            if let Ok(value) = HeaderValue::from_str(&id) {
                response.headers_mut().insert(TRACE_HEADER, value);
            }

            Ok(response)
        }
        .instrument(span)
        .await
    }
}
