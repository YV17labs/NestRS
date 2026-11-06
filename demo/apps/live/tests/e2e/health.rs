use features::testing::assert_a_probe_reads_no_credential;

use super::harness::boot;

#[tokio::test]
async fn a_probe_reads_no_credential() {
    assert_a_probe_reads_no_credential(&boot().await).await;
}
