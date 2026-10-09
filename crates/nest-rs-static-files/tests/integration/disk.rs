//! Covers `src/disk.rs` — every file is resolved through its links and held
//! under the root, a directory answers with its index or not at all, and a
//! file changed on disk is served as it now is.

use nest_rs_static_files::StaticFilesConfig;
use nest_rs_testing::LogCapture;

use crate::{SCRIPT, is_problem_404, serve, site, text, write};

#[cfg(unix)]
#[tokio::test]
async fn a_link_leaving_the_root_is_refused_and_reported() {
    use std::os::unix::fs::symlink;

    let dir = site();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "passwd", "root:x:0:0\n");
    symlink(outside.path().join("passwd"), dir.path().join("passwd")).unwrap();
    symlink(outside.path(), dir.path().join("elsewhere")).unwrap();
    symlink(dir.path().join(".env"), dir.path().join("config.txt")).unwrap();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;
    let logs = LogCapture::install();

    for link in ["/passwd", "/elsewhere/passwd", "/config.txt"] {
        assert!(
            is_problem_404(app.http().get(link).send().await).await,
            "{link} resolves outside what the root serves"
        );
    }
    let escapes = logs.find(
        nest_rs_static_files::TARGET,
        "a link under the static files' root resolves outside what it serves",
    );
    assert_eq!(escapes.len(), 3, "{:#?}", logs.events());
    assert!(escapes.iter().all(|event| event.level == "warn"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_staying_under_the_root_is_served() {
    use std::os::unix::fs::symlink;

    let dir = site();
    symlink(
        dir.path().join("assets/app.js"),
        dir.path().join("latest.js"),
    )
    .unwrap();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let resp = app.http().get("/latest.js").send().await;
    resp.assert_status_is_ok();
    assert_eq!(text(resp).await, SCRIPT);
}

#[tokio::test]
async fn a_directory_answers_with_its_index_and_never_a_listing() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let docs = app.http().get("/docs").send().await;
    docs.assert_status_is_ok();
    docs.assert_content_type("text/html; charset=utf-8");
    assert!(text(docs).await.contains("<title>Publish docs</title>"));

    assert!(is_problem_404(app.http().get("/empty").send().await).await);
    assert!(is_problem_404(app.http().get("/assets").send().await).await);
    assert!(
        is_problem_404(app.http().get("/assets/app.js/x").send().await).await,
        "a file is no directory",
    );
}

#[tokio::test]
async fn a_file_changed_on_disk_is_served_as_it_now_is() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    write(dir.path(), "assets/new.js", "export {};\n");
    let resp = app.http().get("/assets/new.js").send().await;
    resp.assert_status_is_ok();
    assert_eq!(text(resp).await, "export {};\n");
}

#[tokio::test]
async fn a_large_file_is_streamed_whole_and_in_part() {
    let dir = site();
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.path().join("assets/blob.bin"), &bytes).unwrap();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let whole = app.http().get("/assets/blob.bin").send().await;
    whole.assert_status_is_ok();
    whole.assert_header("content-length", bytes.len().to_string());
    assert_eq!(whole.0.into_body().into_vec().await.unwrap(), bytes);

    let part = app
        .http()
        .get("/assets/blob.bin")
        .header("range", "bytes=150000-150009")
        .send()
        .await;
    part.assert_status(poem::http::StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        part.0.into_body().into_vec().await.unwrap(),
        &bytes[150_000..150_010]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_fifo_under_the_root_is_no_file_and_never_blocks() {
    let dir = site();
    let fifo = std::ffi::CString::new(
        dir.path()
            .join("assets/pipe")
            .into_os_string()
            .into_encoded_bytes(),
    )
    .unwrap();
    // SAFETY: a NUL-terminated path the call only reads.
    #[expect(unsafe_code, reason = "std has no mkfifo")]
    let made = unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) };
    assert_eq!(made, 0, "mkfifo");
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let answered = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        app.http().get("/assets/pipe").send(),
    )
    .await
    .expect("opening a FIFO must not wait for a writer");
    assert!(is_problem_404(answered).await);
}
