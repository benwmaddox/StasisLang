use std::hint::black_box;
use std::time::Instant;

use stasis_dynload::{
    plan_cross_atlas_frame, CrossAtlasBinding, CrossAtlasInstance, CrossAtlasProfile,
    CrossAtlasQueueMode,
};

const FIXTURE_COUNT: usize = 4_096;
const SAMPLE_COUNT: usize = 31;
const ITERATIONS_PER_SAMPLE: usize = 1_000;

fn interleaved_single_domain() -> Vec<CrossAtlasInstance> {
    (0..FIXTURE_COUNT)
        .map(|index| CrossAtlasInstance {
            destination: [(index % 128) as f32, (index / 128) as f32, 16.0, 16.0],
            uv_crop: [0.0, 0.0, 1.0, 1.0],
            pivot: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            tint_rgba: 0xffff_ffff,
            resource_id: (index % 8) as u32,
            order: index as u32,
            clip_id: 0,
            binding_domain_id: 1,
            material_id: 0,
            blend_mode: 0,
            filter_mode: 0,
            pass_id: 0,
            flags: 0,
            feature_flags: 0,
        })
        .collect()
}

fn asset_major_single_domain() -> Vec<CrossAtlasInstance> {
    let mut instances = interleaved_single_domain();
    for (index, instance) in instances.iter_mut().enumerate() {
        instance.resource_id = (index / (FIXTURE_COUNT / 8)) as u32;
    }
    instances
}

fn interleaved_two_domain_spill() -> Vec<CrossAtlasInstance> {
    let mut instances = interleaved_single_domain();
    for instance in &mut instances {
        instance.binding_domain_id = if instance.resource_id < 4 { 1 } else { 2 };
    }
    instances
}

fn binding_number(binding: CrossAtlasBinding) -> u8 {
    match binding {
        CrossAtlasBinding::Conventional => 0,
        CrossAtlasBinding::MegaAtlas => 1,
        CrossAtlasBinding::TextureArray => 2,
        CrossAtlasBinding::Bindless => 3,
    }
}

fn print_profile(
    fixture_name: &str,
    profile: CrossAtlasProfile,
    instances: &[CrossAtlasInstance],
    last: bool,
) {
    let mut raw_samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut final_plan = None;
    let mut guard = 0_u32;
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        for _ in 0..ITERATIONS_PER_SAMPLE {
            let plan = plan_cross_atlas_frame(
                black_box(profile),
                black_box(instances),
                FIXTURE_COUNT,
                false,
            );
            guard ^= black_box(plan.order_hash);
            final_plan = Some(plan);
        }
        let per_iteration = started.elapsed().as_nanos() / ITERATIONS_PER_SAMPLE as u128;
        raw_samples.push(per_iteration.min(u128::from(u64::MAX)) as u64);
    }
    let mut sorted = raw_samples.clone();
    sorted.sort_unstable();
    let plan = final_plan.expect("benchmark executes at least one plan");
    print!(
        "    {{\"fixture\":\"{}\",\"profile\":\"{}\",\"binding\":{},\"fixture_instances\":{},",
        fixture_name,
        profile.name,
        binding_number(profile.binding),
        FIXTURE_COUNT
    );
    print!(
        "\"planner_ns\":{{\"min\":{},\"p50\":{},\"p95\":{},\"max\":{}}},",
        sorted[0], sorted[15], sorted[29], sorted[30]
    );
    print!("\"raw_samples_ns\":[");
    for (index, sample) in raw_samples.iter().enumerate() {
        print!("{}{sample}", if index == 0 { "" } else { "," });
    }
    print!(
        "],\"modeled\":{{\"baseline_upload_calls\":{},\"baseline_upload_bytes\":{},\"baseline_texture_binds\":{},\"baseline_draws\":{},\"prototype_upload_calls\":{},\"prototype_upload_bytes\":{},\"prototype_texture_binds\":{},\"prototype_draws\":{},\"queue_submissions\":",
        plan.baseline.upload_calls,
        plan.baseline.upload_bytes,
        plan.baseline.texture_binds,
        plan.baseline.draw_calls,
        plan.prototype.upload_calls,
        plan.prototype.upload_bytes,
        plan.prototype.texture_binds,
        plan.prototype.draw_calls
    );
    match plan.prototype.queue_submissions {
        Some(value) => print!("{value}"),
        None => print!("null"),
    }
    println!(
        "}},\"gpu_frame_time\":null,\"guard\":{guard}}}{}",
        if last { "" } else { "," }
    );
}

fn main() {
    let fixtures = [
        ("interleaved_single_domain", interleaved_single_domain()),
        ("asset_major_single_domain", asset_major_single_domain()),
        (
            "interleaved_two_domain_spill",
            interleaved_two_domain_spill(),
        ),
    ];
    let profiles = [
        CrossAtlasProfile {
            name: "desktop_native_bindless",
            binding: CrossAtlasBinding::Bindless,
            max_instances_per_draw: 65_535,
            supported_feature_flags: 0,
            one_frame_upload: true,
            queue_mode: CrossAtlasQueueMode::One,
        },
        CrossAtlasProfile {
            name: "native_mega_atlas",
            binding: CrossAtlasBinding::MegaAtlas,
            max_instances_per_draw: 4_096,
            supported_feature_flags: 0,
            one_frame_upload: true,
            queue_mode: CrossAtlasQueueMode::One,
        },
        CrossAtlasProfile {
            name: "android_texture_array",
            binding: CrossAtlasBinding::TextureArray,
            max_instances_per_draw: 4_096,
            supported_feature_flags: 0,
            one_frame_upload: true,
            queue_mode: CrossAtlasQueueMode::One,
        },
        CrossAtlasProfile {
            name: "webgl2_texture_array",
            binding: CrossAtlasBinding::TextureArray,
            max_instances_per_draw: 1_024,
            supported_feature_flags: 0,
            one_frame_upload: true,
            queue_mode: CrossAtlasQueueMode::One,
        },
        CrossAtlasProfile {
            name: "canvas_conventional",
            binding: CrossAtlasBinding::Conventional,
            max_instances_per_draw: 4_096,
            supported_feature_flags: 0,
            one_frame_upload: false,
            queue_mode: CrossAtlasQueueMode::Unavailable,
        },
    ];
    println!("{{");
    println!("  \"schema\":1,");
    println!("  \"method\":{{\"samples\":31,\"iterations_per_sample\":1000,\"quantiles\":\"nearest rank after ascending sort: p50=index15, p95=index29\",\"timer\":\"std::time::Instant elapsed\",\"gpu_measurement\":\"unavailable\"}},");
    println!("  \"profiles\":[");
    let total = fixtures.len() * profiles.len();
    let mut emitted = 0;
    for (fixture_name, instances) in &fixtures {
        for profile in profiles {
            emitted += 1;
            print_profile(fixture_name, profile, instances, emitted == total);
        }
    }
    println!("  ]");
    println!("}}");
}
