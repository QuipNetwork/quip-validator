#[path = "support/ising_topology_fixture.rs"]
mod ising_topology_fixture_support;

use serde_json::Value;

#[test]
fn checked_in_fixture_matches_generator() {
    let checked_in: Value =
        serde_json::from_str(include_str!("../../../docs/fixtures/ising-topology.json"))
            .expect("checked-in fixture is valid JSON");

    assert_eq!(
        checked_in,
        ising_topology_fixture_support::generate_fixture()
    );
}
