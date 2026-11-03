//! Bounded static frontend access without following symlinks.

use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

fn read_file(root: &Path, relative: &Path) -> std::io::Result<Vec<u8>> {
    let mut dir = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::other("invalid path"));
        };
        if components.peek().is_some() {
            dir = openat(
                &dir,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
        } else {
            let fd = openat(
                &dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            let file = std::fs::File::from(fd);
            if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_FILE_BYTES {
                return Err(std::io::Error::other("invalid file"));
            }
            let mut data = Vec::new();
            file.take(MAX_FILE_BYTES + 1).read_to_end(&mut data)?;
            if data.len() as u64 > MAX_FILE_BYTES {
                return Err(std::io::Error::other("file too large"));
            }
            return Ok(data);
        }
    }
    Err(std::io::Error::other("empty path"))
}

pub(crate) fn index_available(root: &Path) -> bool {
    read_file(root, Path::new("index.html")).is_ok()
}

pub async fn serve(root: PathBuf, request: Request) -> Response {
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = request.uri().path().trim_start_matches('/');
    if path.contains('%') || path.split('/').any(|part| part == "." || part == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let asset = path.starts_with("assets/");
    let relative = if asset {
        PathBuf::from(path)
    } else {
        PathBuf::from("index.html")
    };
    let mime = if asset {
        match relative
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
        {
            "js" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "woff2" => "font/woff2",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        }
    } else {
        "text/html; charset=utf-8"
    };
    let data = tokio::task::spawn_blocking(move || read_file(&root, &relative)).await;
    let Ok(Ok(data)) = data else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(if request.method() == axum::http::Method::HEAD {
        Body::empty()
    } else {
        Body::from(data)
    });
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(if asset {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    response
}
