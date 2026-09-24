//! Run the production atlas-affinity planner from a small TSV snapshot.
//!
//! The input is emitted by `tools/atlas_affinity_preview.py`; this example
//! contains no placement or optimization logic of its own.

use std::fmt::Write as _;
use std::io::{self, Read};

use stasis_dynload::{
    plan_atlas_affinity, AtlasBaselinePlacement, AtlasCompatibilityKey, AtlasMemoryBudget,
    AtlasMetrics, AtlasPageExtent, AtlasPairWeight, AtlasPlanningInput, AtlasSpriteDescriptor,
};

fn parse_number<T: std::str::FromStr>(value: &str, field: &str, line: usize) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("line {line}: invalid {field}: {value:?}"))
}

fn parse_optional_u64(value: &str, field: &str, line: usize) -> Result<Option<u64>, String> {
    if value == "-" {
        Ok(None)
    } else {
        parse_number(value, field, line).map(Some)
    }
}

fn parse_optional_u32(value: &str, field: &str, line: usize) -> Result<Option<u32>, String> {
    if value == "-" {
        Ok(None)
    } else {
        parse_number(value, field, line).map(Some)
    }
}

fn parse_compatibility(fields: &[&str], line: usize) -> Result<AtlasCompatibilityKey, String> {
    if fields.len() != 5 {
        return Err(format!("line {line}: compatibility requires five fields"));
    }
    Ok(AtlasCompatibilityKey {
        group_id: parse_optional_u64(fields[0], "group id", line)?,
        format: parse_optional_u32(fields[1], "format", line)?,
        sampler: parse_optional_u32(fields[2], "sampler", line)?,
        color_space: parse_optional_u32(fields[3], "color space", line)?,
        backend: parse_optional_u32(fields[4], "backend", line)?,
    })
}

fn json_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "null".to_string(), |value| value.to_string())
}

fn compatibility_json(value: &AtlasCompatibilityKey) -> String {
    format!(
        "{{\"group_id\":{},\"format\":{},\"sampler\":{},\"color_space\":{},\"backend\":{}}}",
        json_optional(value.group_id),
        json_optional(value.format),
        json_optional(value.sampler),
        json_optional(value.color_space),
        json_optional(value.backend),
    )
}

fn metrics_json(value: &AtlasMetrics) -> String {
    format!(
        "{{\"baseline_page_count\":{},\"candidate_page_count\":{},\"moved_sprite_count\":{},\"baseline_cut_weight\":{},\"candidate_cut_weight\":{},\"cut_weight_saved\":{},\"candidate_texture_bytes\":{},\"final_device_bytes\":{},\"peak_device_bytes\":{}}}",
        value.baseline_page_count,
        value.candidate_page_count,
        value.moved_sprite_count,
        value.baseline_cut_weight,
        value.candidate_cut_weight,
        value.cut_weight_saved,
        value.candidate_texture_bytes,
        value.final_device_bytes,
        value.peak_device_bytes,
    )
}

fn fallback_name(value: &stasis_dynload::AtlasFallbackReason) -> &'static str {
    use stasis_dynload::AtlasFallbackReason as Reason;
    match value {
        Reason::StaleGeneration => "stale_generation",
        Reason::EmptyInput => "empty_input",
        Reason::TooManySprites => "too_many_sprites",
        Reason::TooManyPages => "too_many_pages",
        Reason::TooManyPairWeights => "too_many_pair_weights",
        Reason::NoPages => "no_pages",
        Reason::DuplicateSpriteId => "duplicate_sprite_id",
        Reason::DuplicatePageId => "duplicate_page_id",
        Reason::DuplicateBaselinePlacement => "duplicate_baseline_placement",
        Reason::MissingBaselinePlacement => "missing_baseline_placement",
        Reason::UnknownCompatibility => "unknown_compatibility",
        Reason::InvalidSpriteExtent => "invalid_sprite_extent",
        Reason::InvalidPageExtent => "invalid_page_extent",
        Reason::InvalidPageAllocation => "invalid_page_allocation",
        Reason::InvalidBaselinePlacement => "invalid_baseline_placement",
        Reason::OverlappingBaselinePlacements => "overlapping_baseline_placements",
        Reason::IncompatibleBaselinePage => "incompatible_baseline_page",
        Reason::UnknownPairSprite => "unknown_pair_sprite",
        Reason::InvalidPairWeight => "invalid_pair_weight",
        Reason::DuplicatePairWeight => "duplicate_pair_weight",
        Reason::Overflow => "overflow",
        Reason::NoFeasiblePlacement => "no_feasible_placement",
        Reason::InvalidMemoryAccounting => "invalid_memory_accounting",
        Reason::MemoryBudgetExceeded => "memory_budget_exceeded",
        Reason::NonImproving => "non_improving",
        Reason::ClusterSearchLimitExceeded => "cluster_search_limit_exceeded",
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn run() -> Result<String, String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| format!("failed to read planner input: {error}"))?;

    let mut current_generation = None;
    let mut baseline_generation = None;
    let mut budget = None;
    let mut pages = Vec::<AtlasPageExtent>::new();
    let mut sprites = Vec::<AtlasSpriteDescriptor>::new();
    let mut baseline_placements = Vec::<AtlasBaselinePlacement>::new();
    let mut pair_weights = Vec::<AtlasPairWeight>::new();
    let mut found_header = false;

    for (index, raw_line) in text.lines().enumerate() {
        let line = index + 1;
        let raw_line = raw_line.trim_end_matches('\r');
        if raw_line.is_empty() || raw_line.starts_with('#') {
            continue;
        }
        if !found_header {
            if raw_line != "ATLAS_AFFINITY_PREVIEW_V1" {
                return Err(format!("line {line}: expected ATLAS_AFFINITY_PREVIEW_V1"));
            }
            found_header = true;
            continue;
        }
        let fields = raw_line.split('\t').collect::<Vec<_>>();
        match fields[0] {
            "meta" if fields.len() == 6 => {
                current_generation = Some(parse_number(fields[1], "current generation", line)?);
                baseline_generation = Some(parse_number(fields[2], "baseline generation", line)?);
                budget = Some(AtlasMemoryBudget {
                    current_device_bytes: parse_number(fields[3], "current device bytes", line)?,
                    max_final_bytes: parse_number(fields[4], "maximum final bytes", line)?,
                    max_peak_bytes: parse_number(fields[5], "maximum peak bytes", line)?,
                });
            }
            "page" if fields.len() == 12 => {
                pages.push(AtlasPageExtent {
                    id: parse_number(fields[1], "page id", line)?,
                    compatibility: parse_compatibility(&fields[7..12], line)?,
                    width: parse_number(fields[2], "page width", line)?,
                    height: parse_number(fields[3], "page height", line)?,
                    usable_origin_x: parse_number(fields[4], "usable origin x", line)?,
                    usable_origin_y: parse_number(fields[5], "usable origin y", line)?,
                    allocation_bytes: parse_number(fields[6], "page allocation bytes", line)?,
                });
            }
            "sprite" if fields.len() == 10 => {
                sprites.push(AtlasSpriteDescriptor {
                    id: parse_number(fields[1], "sprite id", line)?,
                    width: parse_number(fields[2], "sprite width", line)?,
                    height: parse_number(fields[3], "sprite height", line)?,
                    padding: parse_number(fields[4], "sprite padding", line)?,
                    compatibility: parse_compatibility(&fields[5..10], line)?,
                });
            }
            "baseline" if fields.len() == 8 => {
                baseline_placements.push(AtlasBaselinePlacement {
                    sprite_id: parse_number(fields[1], "baseline sprite id", line)?,
                    page_id: parse_number(fields[2], "baseline page id", line)?,
                    x: parse_number(fields[3], "baseline x", line)?,
                    y: parse_number(fields[4], "baseline y", line)?,
                    width: parse_number(fields[5], "baseline width", line)?,
                    height: parse_number(fields[6], "baseline height", line)?,
                    padding: parse_number(fields[7], "baseline padding", line)?,
                });
            }
            "edge" if fields.len() == 4 => {
                pair_weights.push(AtlasPairWeight {
                    from_sprite_id: parse_number(fields[1], "edge source sprite id", line)?,
                    to_sprite_id: parse_number(fields[2], "edge target sprite id", line)?,
                    weight: parse_number(fields[3], "edge weight", line)?,
                });
            }
            record => {
                return Err(format!(
                    "line {line}: malformed or unknown record {record:?}"
                ));
            }
        }
    }

    let input = AtlasPlanningInput {
        current_generation: current_generation.ok_or("missing meta record")?,
        baseline_generation: baseline_generation.ok_or("missing meta record")?,
        sprites: &sprites,
        pages: &pages,
        baseline_placements: &baseline_placements,
        pair_weights: &pair_weights,
        budget: budget.ok_or("missing meta record")?,
    };
    let result = plan_atlas_affinity(&input);
    let fallback = result
        .fallback_reason
        .as_ref()
        .map(|reason| json_string(fallback_name(reason)))
        .unwrap_or_else(|| "null".to_string());
    let baseline_metrics = result
        .baseline_metrics
        .as_ref()
        .map(metrics_json)
        .unwrap_or_else(|| "null".to_string());
    let candidate_metrics = result
        .candidate_metrics
        .as_ref()
        .map(metrics_json)
        .unwrap_or_else(|| "null".to_string());

    let accepted = result.plan.is_some();
    let plan_json = result.plan.map_or_else(
        || "null".to_string(),
        |plan| {
            let pages_json = plan
                .pages
                .iter()
                .map(|page| {
                    format!(
                        "{{\"id\":{},\"width\":{},\"height\":{},\"usable_origin_x\":{},\"usable_origin_y\":{},\"allocation_bytes\":{},\"sprite_count\":{},\"compatibility\":{}}}",
                        page.id,
                        page.width,
                        page.height,
                        page.usable_origin_x,
                        page.usable_origin_y,
                        page.allocation_bytes,
                        page.sprite_count,
                        compatibility_json(&page.compatibility),
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let placements_json = plan
                .placements
                .iter()
                .map(|placement| {
                    format!(
                        "{{\"sprite_id\":{},\"page_id\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"allocation_x\":{},\"allocation_y\":{},\"allocation_width\":{},\"allocation_height\":{}}}",
                        placement.sprite_id,
                        placement.page_id,
                        placement.x,
                        placement.y,
                        placement.width,
                        placement.height,
                        placement.allocation_x,
                        placement.allocation_y,
                        placement.allocation_width,
                        placement.allocation_height,
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"metrics\":{},\"pages\":[{}],\"placements\":[{}]}}",
                metrics_json(&plan.metrics),
                pages_json,
                placements_json,
            )
        },
    );

    Ok(format!(
        "{{\"schema\":\"atlas-affinity-plan/v1\",\"accepted\":{},\"fallback_reason\":{},\"baseline_metrics\":{},\"candidate_metrics\":{},\"plan\":{}}}",
        accepted,
        fallback,
        baseline_metrics,
        candidate_metrics,
        plan_json,
    ))
}

fn main() {
    match run() {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("atlas_affinity_preview: {error}");
            std::process::exit(2);
        }
    }
}
