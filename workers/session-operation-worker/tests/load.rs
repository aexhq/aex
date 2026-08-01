//! Bounded local load contracts for continuation sharding.

use session_operation_worker::due_shard;

#[test]
fn due_shard_projection_is_total_at_batch_load() {
    let projected = (0..100_000)
        .map(|index| due_shard(&format!("work-{index}"), 256).expect("shard"))
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), 100_000);
    assert!(projected.into_iter().all(|shard| shard < 256));
}
