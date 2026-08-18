//! Prints the digest of the embedded tool manifest v1 (used by tools/gen.sh to pin it).
fn main() {
    let digest = aex_contracts::tools::manifest_digest(aex_contracts::tools::manifest_v1());
    print!("{}", *digest);
}
