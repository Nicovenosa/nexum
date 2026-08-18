const NOTICE: &str = include_str!("../../NOTICE");
const PACKAGED_NOTICE: &str = include_str!("../NOTICE");

#[test]
fn runtime_notice_attributes_peri_konghayao_and_nexum_changes() {
    assert!(NOTICE.contains("Peri"));
    assert!(NOTICE.contains("KonghaYao"));
    assert!(NOTICE.contains("Apache License, Version 2.0"));
    assert!(NOTICE.contains("Nexum modifications"));
    assert_eq!(PACKAGED_NOTICE, NOTICE, "the crate package ships the runtime notice");
}
