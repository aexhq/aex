//! Cross-boundary Data API page-limit evidence.

use aex_finance_aurora::row::{DATA_API_MAX_RESULT_BYTES, DATA_API_MAX_ROW_BYTES, RowPage};

#[test]
fn decoded_pages_compose_both_provider_byte_bounds() {
    let page = RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES]], Some("next".into()))
        .expect("boundary row is admitted");
    assert_eq!(page.next_key.as_deref(), Some("next"));
    assert!(RowPage::new(vec![vec![0; DATA_API_MAX_ROW_BYTES + 1]], None).is_err());
    assert!(
        RowPage::new(
            vec![
                vec![0; DATA_API_MAX_ROW_BYTES];
                DATA_API_MAX_RESULT_BYTES / DATA_API_MAX_ROW_BYTES + 1
            ],
            None
        )
        .is_err()
    );
}
