//! H-BOUNDARY B1-B9 against a real `MicroVM`.
//!
//! Every case here needs a hypervisor. None of it can be earned in this run
//! (OD-07: nothing is deployed, published or credentialed), so each test **fails
//! loudly** when the live lane selects it. That is deliberate: a declared,
//! failing live case is honest unavailable evidence, and a self-skip would make
//! the release composition look available when it is not.
//!
//! The unit-level halves of these controls already pass in the default lane:
//!
//! | Control | Unit-level falsifying test | What still needs a VM |
//! | --- | --- | --- |
//! | B1 no execution role | `aex-hands-control-aws` `no_execution_role_can_be_threaded_through_a_launch` | IMDS and `sts get-caller-identity` from inside the guest |
//! | B2 no private route | `aex-hands-control-aws` launch-shape golden | resolving a regional private endpoint from the guest |
//! | B3 no shell ingress | `aex-hands-control-aws` `no_shell_ingress_action_is_in_the_runtime_role` | the deployed IAM policy document |
//! | B4 no managed secret | `aex-hands-control-aws` `the_run_hook_payload_key_set_is_closed_and_sorted` | a rootfs and process-environment canary scan |
//! | B5 token never leaves trusted memory | `aex-hands-control-aws` `an_endpoint_token_never_renders_its_secret` | the proxy actually stripping the header |
//! | B6 no cloud authority in the binary | `aex-hands-agent` `no_cloud_authority` | `ldd` on the built `aarch64` binary |
//! | B7 whole-VM ceilings | shape golden table | saturating CPU, memory, disk, forks and connections |
//! | B8 no guest-reported fact is billable | `aex-hands-agent` `no_guest_billing` | a forged terminal from a real hostile guest |
//! | B9 cross-tenant isolation | none: it is the hypervisor boundary | two generations, two workspaces |

/// The message every unavailable live case fails with.
const UNAVAILABLE: &str =
    "live evidence requires a real MicroVM in a deployed plane; this run deploys nothing (OD-07)";

#[test]
fn b1_the_guest_reaches_no_instance_credential() {
    panic!("{UNAVAILABLE}: curl IMDSv1 and IMDSv2 from guest root and assert no credential");
}

#[test]
fn b2_the_guest_reaches_no_private_aex_route() {
    panic!("{UNAVAILABLE}: resolve and connect to a regional private endpoint from the guest");
}

#[test]
fn b4_no_planted_canary_secret_appears_anywhere_in_the_guest() {
    panic!("{UNAVAILABLE}: scan /proc/1/environ, the rootfs and every spawned environment");
}

#[test]
fn b5_the_guest_never_observes_the_endpoint_auth_header() {
    panic!("{UNAVAILABLE}: assert the proxy strips X-aws-proxy-auth before the guest sees it");
}

#[test]
fn b6_the_agent_binary_is_static_and_carries_no_sdk() {
    panic!("{UNAVAILABLE}: ldd reports not a dynamic executable; file reports aarch64");
}

#[test]
fn b7_hostile_root_workloads_are_contained_by_the_vm() {
    panic!("{UNAVAILABLE}: fork bomb, output bomb, FD bomb, disk fill and connection saturation");
}

#[test]
fn b9_two_generations_cannot_see_each_other() {
    panic!("{UNAVAILABLE}: two workspaces, two generations, no shared filesystem or process view");
}

#[test]
fn the_image_boots_on_every_offered_shape() {
    panic!("{UNAVAILABLE}: boot all five base variants and the three browser variants");
}

#[test]
fn the_language_free_variant_passes_the_whole_protocol_and_filesystem_suite() {
    panic!("{UNAVAILABLE}: prove no AEX path invokes an interpreter by removing every one");
}

#[test]
fn the_installed_package_set_equals_the_nevra_lockfile() {
    panic!("{UNAVAILABLE}: rpm -qa inside a booted guest against image.lock.json");
}
