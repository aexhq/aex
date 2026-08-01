//! Bounded local load contracts for branch-key generation reads.

use regional_secret_key_admin::KeyAdmin;

#[test]
fn current_generation_reads_remain_stable_under_load() {
    let admin = KeyAdmin::new("workspace").expect("admin");
    for _ in 0..100_000 {
        assert_eq!(admin.current_generation(), None);
    }
}
