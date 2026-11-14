//! Emit the shared Unicode corpus through the native evaluator for actual WASM comparison.
use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeSnapshot, SnapshotEnvironment, ToolchainContract,
    VirtualPath,
};

fn main() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../resources/config-schema/unicode-values.json"
    ))
    .unwrap();
    let results: Vec<_> = cases.iter().map(|case| {
        let snapshot = ConfigTreeSnapshot::from_entries(case["entries"].as_object().unwrap().iter().map(|(path, source)| {
            ConfigEntry::new(VirtualPath::parse(path).unwrap(), if path.ends_with(".md") { ConfigContentType::Text } else { ConfigContentType::Decodal }, source.as_str().unwrap()).unwrap()
        })).unwrap();
        let contract = ToolchainContract::new(1, vec![VirtualPath::parse("main.dcdl").unwrap()], 1);
        let environment = SnapshotEnvironment::new(snapshot.clone());
        let evaluation = environment.evaluate_contract(&contract);
        if case["error"].as_bool() == Some(true) {
            assert!(evaluation.is_err(), "{}", case["name"]);
        } else {
            assert_eq!(evaluation.as_ref().unwrap().projections[0].data_json, case["expected"], "{}", case["name"]);
        }
        let analysis = environment.analyze(&VirtualPath::parse("main.dcdl").unwrap(), None);
        let formatted = environment.format(snapshot.get(&VirtualPath::parse("main.dcdl").unwrap()).unwrap().content.as_str());
        serde_json::json!({"name":case["name"], "snapshot":snapshot, "contract":contract, "evaluation":evaluation, "analysis":analysis, "formatted":formatted})
    }).collect();
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
