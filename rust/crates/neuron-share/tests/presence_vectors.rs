//! The presence tracker against the apps' reducer: `tests/fixtures/presence-vectors.json` is what
//! `ui/neuron-mobile/test/fixtures/presence-vectors.ts` prints (the session reducer and
//! `presenceOf` in TypeScript), step by step.

use neuron_share::presence::PresenceTracker;
use serde_json::Value;

#[test]
fn presence_matches_the_typescript_reducer() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/presence-vectors.json")).unwrap();
    let mut checked = 0;
    for scenario in vectors["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let mut tracker = PresenceTracker::new(scenario["fallback_cwd"].as_str().unwrap());
        for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(snapshot) = step.get("snapshot") {
                tracker.snapshot(snapshot["seq"].as_u64().unwrap(), &snapshot["state"]);
            } else {
                let action = &step["action"];
                tracker.action(action["seq"].as_u64().unwrap(), &action["action"]);
            }
            let mut presence = serde_json::to_value(tracker.presence(0)).unwrap();
            presence.as_object_mut().unwrap().remove("updated_at_ms");
            assert_eq!(presence, step["expect"], "{name}, step {index}: {step}");
            checked += 1;
        }
    }
    assert!(checked > 40, "the vectors cover a whole session");
}
