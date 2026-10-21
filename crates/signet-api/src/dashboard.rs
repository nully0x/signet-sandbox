use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The dashboard SPA: `ui/` builds it, the API embeds the bundle.
/// Debug builds read `ui/dist` from disk; release builds embed.
#[derive(RustEmbed)]
#[folder = "../../ui/dist/"]
struct Ui;

/// Serve embedded dashboard assets; unknown paths fall back to
/// index.html so client-side routing works.
pub async fn dashboard(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    serve_asset(requested)
        .or_else(|| serve_asset("index.html"))
        .unwrap_or_else(not_built)
}

fn serve_asset(path: &str) -> Option<Response> {
    let file = Ui::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        (
            [(axum::http::header::CONTENT_TYPE, mime.as_ref().to_owned())],
            file.data.into_owned(),
        )
            .into_response(),
    )
}

fn not_built() -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        "dashboard assets not built — run `npm --prefix ui run build`",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_paths_fall_back_to_the_index_page() {
        let response = dashboard("/environments/x".parse().unwrap()).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn root_serves_the_dashboard() {
        let response = dashboard("/".parse().unwrap()).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
