use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::{ContextDeliveryRecord, ContextDeliveryV1, ContextIndex};

use super::{
    delivery_record, dependency_neighborhood, file_report, finish_map_delivery, impact_report,
    normalize_delivery_options, package_name_for_path,
};

pub const MAP_DELIVERY_FIELDS: &[&str] = &[
    "path",
    "sha256",
    "line_range",
    "reason",
    "provenance",
    "record_type",
    "name",
    "kind",
    "visibility",
    "module",
    "package",
    "manifest_path",
    "root_path",
    "direction",
    "depth",
    "from_package",
    "from_root_path",
    "to_package",
    "to_root_path",
    "from_path",
    "to_path",
    "dependency_name",
    "evidence",
    "test_count",
    "candidate_paths",
    "basis",
    "confidence",
    "severity",
    "detail",
    "command",
    "role",
    "ready",
    "review_priority",
    "workspace_member",
    "source_files",
    "test_files",
];

#[derive(Debug, Clone, Serialize)]
pub struct MapDeliveryOptions {
    pub path_prefix: Option<String>,
    pub package: Option<String>,
    pub fields: Vec<String>,
    pub max_depth: usize,
    #[serde(skip)]
    pub limit: usize,
    #[serde(skip)]
    pub cursor: Option<String>,
    #[serde(skip)]
    pub budget_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMap {
    pub generated_at: String,
    pub workspace: String,
    #[serde(default)]
    pub topology_provenance: String,
    pub source_files: usize,
    pub test_files: usize,
    pub docs_files: usize,
    pub config_files: usize,
    #[serde(default)]
    pub script_files: usize,
    pub asset_files: usize,
    pub modules: Vec<ModuleEntry>,
    pub symbols: Vec<MapSymbol>,
    pub dependencies: Vec<Dependency>,
    pub packages: Vec<PackageEntry>,
    pub package_dependencies: Vec<PackageDependency>,
    pub entrypoints: Vec<EntryPoint>,
    pub tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub lines: usize,
    pub symbols: usize,
    pub public_symbols: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapSymbol {
    pub name: String,
    pub kind: String,
    pub visibility: String,
    pub module: String,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub from_path: String,
    pub to_path: String,
    pub kind: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEntry {
    pub name: String,
    pub root_path: String,
    pub manifest_path: String,
    pub workspace_member: bool,
    pub source_files: usize,
    pub test_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDependency {
    pub from_package: String,
    pub from_root_path: String,
    pub to_package: String,
    pub to_root_path: String,
    pub dependency_name: String,
    pub kind: String,
    pub manifest_path: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPoint {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestTarget {
    pub path: String,
    pub kind: String,
    pub test_count: usize,
    pub candidate_paths: Vec<String>,
    pub basis: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRisk {
    pub severity: String,
    pub kind: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapSummary {
    pub generated_at: String,
    pub files: usize,
    pub source_files: usize,
    pub test_files: usize,
    pub modules: usize,
    pub symbols: usize,
    pub dependencies: usize,
    pub packages: usize,
    pub package_dependencies: usize,
    pub entrypoints: usize,
    pub tests: usize,
    pub risks: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub module: Option<ModuleEntry>,
    pub symbols: Vec<MapSymbol>,
    pub outgoing_dependencies: Vec<Dependency>,
    pub incoming_dependencies: Vec<Dependency>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolReport {
    pub query: String,
    pub matches: Vec<MapSymbol>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopologyReport {
    pub provenance: String,
    pub packages: Vec<PackageEntry>,
    pub package_dependencies: Vec<PackageDependency>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub changed_path: String,
    pub package: Option<String>,
    pub manifest_path: Option<String>,
    pub direct_dependencies: Vec<Dependency>,
    pub direct_dependents: Vec<Dependency>,
    pub package_dependencies: Vec<PackageDependency>,
    pub package_dependents: Vec<PackageDependency>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
    pub recommended_checks: Vec<String>,
    pub recommendation_basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangePlan {
    pub ready: bool,
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_files: Vec<PlanFile>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
    pub recommended_checks: Vec<String>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub recommendation_basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanFile {
    pub path: String,
    pub role: String,
    pub reason: String,
}

fn delivery_attributes(value: Value, fields: &[String]) -> Result<BTreeMap<String, Value>> {
    let Value::Object(object) = value else {
        bail!("map delivery attributes must be a JSON object");
    };
    let mut attributes = object.into_iter().collect();
    super::project_delivery_fields(fields, &mut attributes);
    Ok(attributes)
}

fn traversal_records(
    index: &ContextIndex,
    map: &ProjectMap,
    seeds: &[String],
    options: &MapDeliveryOptions,
) -> Result<Vec<ContextDeliveryRecord>> {
    dependency_neighborhood(map, seeds, options.max_depth)
        .into_iter()
        .map(|edge| {
            let anchor = if edge.direction == "dependency" {
                edge.dependency.to_path.as_str()
            } else {
                edge.dependency.from_path.as_str()
            };
            Ok(delivery_record(
                index,
                anchor,
                None,
                format!("{} at dependency depth {}", edge.direction, edge.depth),
                "project_map_dependency",
                edge.dependency.evidence.clone(),
                delivery_attributes(
                    json!({
                        "record_type": "dependency_edge",
                        "direction": edge.direction,
                        "depth": edge.depth,
                        "from_path": edge.dependency.from_path,
                        "to_path": edge.dependency.to_path,
                        "kind": edge.dependency.kind,
                        "evidence": edge.dependency.evidence,
                        "package": package_name_for_path(map, anchor),
                    }),
                    &options.fields,
                )?,
            ))
        })
        .collect()
}

pub fn topology_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    map: &ProjectMap,
    options: MapDeliveryOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_delivery_options(map, options)?;
    let report = super::topology_report(map);
    let mut records = Vec::new();
    for package in &report.packages {
        records.push(delivery_record(
            index,
            &package.manifest_path,
            None,
            "Cargo package manifest",
            "project_map_topology",
            report.provenance.clone(),
            delivery_attributes(
                json!({
                    "record_type":"package",
                    "name":package.name,
                    "package":package.name,
                    "root_path":package.root_path,
                    "manifest_path":package.manifest_path,
                    "workspace_member":package.workspace_member,
                    "source_files":package.source_files,
                    "test_files":package.test_files,
                    "provenance":report.provenance,
                }),
                &options.fields,
            )?,
        ));
    }
    for dependency in &report.package_dependencies {
        records.push(delivery_record(
            index,
            &dependency.manifest_path,
            None,
            "package dependency edge",
            "project_map_package_dependency",
            dependency.evidence.clone(),
            delivery_attributes(
                json!({
                    "record_type":"package_dependency",
                    "direction":"dependency",
                    "from_package":dependency.from_package,
                    "from_root_path":dependency.from_root_path,
                    "to_package":dependency.to_package,
                    "to_root_path":dependency.to_root_path,
                    "dependency_name":dependency.dependency_name,
                    "kind":dependency.kind,
                    "manifest_path":dependency.manifest_path,
                    "evidence":dependency.evidence,
                    "package":dependency.from_package,
                    "provenance":report.provenance,
                }),
                &options.fields,
            )?,
        ));
    }
    finish_map_delivery(
        index,
        raw_index_sha256,
        map,
        options.clone(),
        json!({"command":"map topology","options":options}),
        "topology:context-record-canonical-json-asc:v1",
        records,
    )
}

pub fn symbol_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    map: &ProjectMap,
    name: &str,
    exact: bool,
    options: MapDeliveryOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_delivery_options(map, options)?;
    let folded = name.to_ascii_lowercase();
    let mut records = Vec::new();
    for symbol in map.symbols.iter().filter(|symbol| {
        if exact {
            symbol.name == name
        } else {
            symbol.name.to_ascii_lowercase().contains(&folded)
        }
    }) {
        records.push(delivery_record(
            index,
            &symbol.path,
            Some(symbol.line),
            if exact {
                "exact symbol match"
            } else {
                "case-insensitive symbol substring match"
            },
            "symbol_index",
            format!("{}:{}", symbol.path, symbol.line),
            delivery_attributes(
                json!({
                    "record_type": "symbol",
                    "name": symbol.name,
                    "kind": symbol.kind,
                    "visibility": symbol.visibility,
                    "module": symbol.module,
                    "package": package_name_for_path(map, &symbol.path),
                }),
                &options.fields,
            )?,
        ));
    }
    finish_map_delivery(
        index,
        raw_index_sha256,
        map,
        options.clone(),
        json!({"command":"map symbol","name":name,"exact":exact,"options":options}),
        "symbol:context-record-canonical-json-asc:v1",
        records,
    )
}

pub fn file_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    map: &ProjectMap,
    path: &str,
    options: MapDeliveryOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_delivery_options(map, options)?;
    let report = file_report(map, path)?;
    let mut records = Vec::new();
    if let Some(module) = &report.module {
        records.push(delivery_record(
            index,
            &module.path,
            None,
            "indexed file module",
            "project_map_module",
            module.name.clone(),
            delivery_attributes(
                json!({
                    "record_type":"module",
                    "name":module.name,
                    "kind":module.kind,
                    "module":module.name,
                    "source_files":1,
                    "package":package_name_for_path(map, &module.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for symbol in &report.symbols {
        records.push(delivery_record(
            index,
            &symbol.path,
            Some(symbol.line),
            "symbol declared in file",
            "symbol_index",
            symbol.name.clone(),
            delivery_attributes(
                json!({
                    "record_type":"symbol",
                    "name":symbol.name,
                    "kind":symbol.kind,
                    "visibility":symbol.visibility,
                    "module":symbol.module,
                    "package":package_name_for_path(map, &symbol.path),
                }),
                &options.fields,
            )?,
        ));
    }
    records.extend(traversal_records(
        index,
        map,
        std::slice::from_ref(&report.path),
        &options,
    )?);
    for test in &report.related_tests {
        records.push(delivery_record(
            index,
            &test.path,
            None,
            "candidate related test",
            "project_map_test_inference",
            test.basis.clone(),
            delivery_attributes(
                json!({
                    "record_type":"test",
                    "kind":test.kind,
                    "test_count":test.test_count,
                    "candidate_paths":test.candidate_paths,
                    "basis":test.basis,
                    "confidence":test.confidence,
                    "package":package_name_for_path(map, &test.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for risk in &report.risks {
        records.push(delivery_record(
            index,
            &risk.path,
            None,
            "project map risk",
            "project_map_risk",
            risk.kind.clone(),
            delivery_attributes(
                json!({
                    "record_type":"risk",
                    "severity":risk.severity,
                    "kind":risk.kind,
                    "detail":risk.detail,
                    "package":package_name_for_path(map, &risk.path),
                }),
                &options.fields,
            )?,
        ));
    }
    finish_map_delivery(
        index,
        raw_index_sha256,
        map,
        options.clone(),
        json!({"command":"map file","path":report.path,"options":options}),
        "file:context-record-canonical-json-asc:v1",
        records,
    )
}

pub fn impact_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    map: &ProjectMap,
    path: &str,
    options: MapDeliveryOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_delivery_options(map, options)?;
    let report = impact_report(map, path)?;
    let mut records = vec![delivery_record(
        index,
        &report.changed_path,
        None,
        "explicit changed path",
        "map_impact_query",
        report.recommendation_basis.clone(),
        delivery_attributes(
            json!({
                "record_type":"changed_path",
                "package":report.package,
                "manifest_path":report.manifest_path,
                "basis":report.recommendation_basis,
            }),
            &options.fields,
        )?,
    )];
    records.extend(traversal_records(
        index,
        map,
        std::slice::from_ref(&report.changed_path),
        &options,
    )?);
    for dependency in report
        .package_dependencies
        .iter()
        .chain(report.package_dependents.iter())
    {
        records.push(delivery_record(
            index,
            &dependency.manifest_path,
            None,
            "package dependency edge",
            "cargo_metadata",
            dependency.evidence.clone(),
            delivery_attributes(
                json!({
                    "record_type":"package_dependency",
                    "from_path":dependency.from_root_path,
                    "to_path":dependency.to_root_path,
                    "dependency_name":dependency.dependency_name,
                    "kind":dependency.kind,
                    "evidence":dependency.evidence,
                    "package":package_name_for_path(map, &dependency.manifest_path),
                }),
                &options.fields,
            )?,
        ));
    }
    for test in &report.related_tests {
        records.push(delivery_record(
            index,
            &test.path,
            None,
            "candidate related test",
            "project_map_test_inference",
            test.basis.clone(),
            delivery_attributes(
                json!({
                    "record_type":"test",
                    "kind":test.kind,
                    "test_count":test.test_count,
                    "candidate_paths":test.candidate_paths,
                    "basis":test.basis,
                    "confidence":test.confidence,
                    "package":package_name_for_path(map, &test.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for risk in &report.risks {
        records.push(delivery_record(
            index,
            &risk.path,
            None,
            "project map risk",
            "project_map_risk",
            risk.kind.clone(),
            delivery_attributes(
                json!({
                    "record_type":"risk",
                    "severity":risk.severity,
                    "kind":risk.kind,
                    "detail":risk.detail,
                    "package":package_name_for_path(map, &risk.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for command in &report.recommended_checks {
        records.push(delivery_record(
            index,
            &report.changed_path,
            None,
            "recommended validation command",
            "map_impact_recommendation",
            report.recommendation_basis.clone(),
            delivery_attributes(
                json!({
                    "record_type":"recommended_check",
                    "command":command,
                    "package":report.package,
                }),
                &options.fields,
            )?,
        ));
    }
    finish_map_delivery(
        index,
        raw_index_sha256,
        map,
        options.clone(),
        json!({"command":"map impact","path":report.changed_path,"options":options}),
        "impact:context-record-canonical-json-asc:v1",
        records,
    )
}

pub fn plan_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    map: &ProjectMap,
    report: &ChangePlan,
    options: MapDeliveryOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_delivery_options(map, options)?;
    let anchor = report
        .changed_paths
        .first()
        .map(String::as_str)
        .unwrap_or("Cargo.toml");
    let mut records = vec![delivery_record(
        index,
        anchor,
        None,
        "change plan summary",
        "map_plan",
        report.recommendation_basis.clone(),
        delivery_attributes(
            json!({
                "record_type":"plan_summary",
                "ready":report.ready,
                "review_priority":report.review_priority,
                "basis":report.recommendation_basis,
                "package":package_name_for_path(map, anchor),
            }),
            &options.fields,
        )?,
    )];
    for file in &report.impacted_files {
        records.push(delivery_record(
            index,
            &file.path,
            None,
            file.reason.clone(),
            "map_plan_impact",
            file.role.clone(),
            delivery_attributes(
                json!({
                    "record_type":"plan_file",
                    "role":file.role,
                    "reason":file.reason,
                    "package":package_name_for_path(map, &file.path),
                }),
                &options.fields,
            )?,
        ));
    }
    records.extend(traversal_records(
        index,
        map,
        &report.changed_paths,
        &options,
    )?);
    for test in &report.related_tests {
        records.push(delivery_record(
            index,
            &test.path,
            None,
            "candidate related test",
            "project_map_test_inference",
            test.basis.clone(),
            delivery_attributes(
                json!({
                    "record_type":"test",
                    "kind":test.kind,
                    "test_count":test.test_count,
                    "candidate_paths":test.candidate_paths,
                    "basis":test.basis,
                    "confidence":test.confidence,
                    "package":package_name_for_path(map, &test.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for risk in &report.risks {
        records.push(delivery_record(
            index,
            &risk.path,
            None,
            "project map risk",
            "project_map_risk",
            risk.kind.clone(),
            delivery_attributes(
                json!({
                    "record_type":"risk",
                    "severity":risk.severity,
                    "kind":risk.kind,
                    "detail":risk.detail,
                    "package":package_name_for_path(map, &risk.path),
                }),
                &options.fields,
            )?,
        ));
    }
    for (record_type, values) in [
        ("recommended_check", &report.recommended_checks),
        ("blocker", &report.blockers),
        ("warning", &report.warnings),
    ] {
        for value in values {
            records.push(delivery_record(
                index,
                anchor,
                None,
                record_type.replace('_', " "),
                "map_plan",
                report.recommendation_basis.clone(),
                delivery_attributes(json!({
                    "record_type":record_type,
                    "detail":value,
                    "command":if record_type == "recommended_check" { Some(value) } else { None },
                    "package":package_name_for_path(map, anchor),
                }), &options.fields)?,
            ));
        }
    }
    finish_map_delivery(
        index,
        raw_index_sha256,
        map,
        options.clone(),
        json!({"command":"map plan","paths":report.changed_paths,"options":options}),
        "plan:context-record-canonical-json-asc:v1",
        records,
    )
}
