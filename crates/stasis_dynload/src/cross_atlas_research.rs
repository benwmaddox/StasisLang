//! Research-only ordered sprite submission planner.
//!
//! This module is excluded from default builds. It models submission counters
//! and split boundaries without changing a production renderer or render ABI.

use std::mem::size_of;

pub const CROSS_ATLAS_INSTANCE_BYTES: usize = 80;
pub const CROSS_ATLAS_SAFE_MAX_INSTANCES: usize = u32::MAX as usize / CROSS_ATLAS_INSTANCE_BYTES;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossAtlasInstance {
    pub destination: [f32; 4],
    pub uv_crop: [f32; 4],
    pub pivot: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
    pub tint_rgba: u32,
    pub resource_id: u32,
    pub order: u32,
    pub clip_id: u32,
    pub binding_domain_id: u16,
    pub material_id: u16,
    pub blend_mode: u8,
    pub filter_mode: u8,
    pub pass_id: u8,
    pub flags: u8,
    pub feature_flags: u32,
}

const _: [(); CROSS_ATLAS_INSTANCE_BYTES] = [(); size_of::<CrossAtlasInstance>()];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAtlasBinding {
    Conventional,
    MegaAtlas,
    TextureArray,
    Bindless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAtlasQueueMode {
    Unavailable,
    One,
    PerRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAtlasSplitReason {
    FrameStart,
    Texture,
    BindingDomain,
    Clip,
    Pass,
    Material,
    BlendFilter,
    Capacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAtlasFallbackReason {
    None,
    InvalidProfile,
    SafeMaximum,
    UnsupportedFeature,
    UploadFailure,
    OutputCapacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossAtlasProfile {
    pub name: &'static str,
    pub binding: CrossAtlasBinding,
    pub max_instances_per_draw: u32,
    pub supported_feature_flags: u32,
    pub one_frame_upload: bool,
    pub queue_mode: CrossAtlasQueueMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossAtlasRun {
    pub first_instance: u32,
    pub instance_count: u32,
    pub reason_before: CrossAtlasSplitReason,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrossAtlasCounters {
    pub upload_bytes: u64,
    pub upload_calls: u32,
    pub texture_binds: u32,
    pub draw_calls: u32,
    pub pass_changes: u32,
    pub queue_submissions: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossAtlasPlan {
    pub baseline: CrossAtlasCounters,
    pub prototype: CrossAtlasCounters,
    pub runs: Vec<CrossAtlasRun>,
    pub input_count: u32,
    pub order_hash: u32,
    pub prototype_used: bool,
    pub fallback_reason: CrossAtlasFallbackReason,
}

pub const fn cross_atlas_instance_count_is_safe(count: usize) -> bool {
    count <= CROSS_ATLAS_SAFE_MAX_INSTANCES
}

fn mix_u32(mut hash: u32, value: u32) -> u32 {
    for shift in [0, 8, 16, 24] {
        hash ^= (value >> shift) & 0xff;
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

fn order_hash(instances: &[CrossAtlasInstance]) -> u32 {
    instances.iter().fold(2_166_136_261, |hash, instance| {
        mix_u32(mix_u32(hash, instance.order), instance.resource_id)
    })
}

fn same_binding_domain(
    binding: CrossAtlasBinding,
    left: &CrossAtlasInstance,
    right: &CrossAtlasInstance,
) -> bool {
    match binding {
        CrossAtlasBinding::Bindless => true,
        CrossAtlasBinding::MegaAtlas | CrossAtlasBinding::TextureArray => {
            left.binding_domain_id == right.binding_domain_id
        }
        CrossAtlasBinding::Conventional => {
            left.resource_id == right.resource_id
                && left.binding_domain_id == right.binding_domain_id
        }
    }
}

fn split_reason(
    profile: CrossAtlasProfile,
    previous: &CrossAtlasInstance,
    current: &CrossAtlasInstance,
    run_length: u32,
) -> Option<CrossAtlasSplitReason> {
    if run_length >= profile.max_instances_per_draw {
        return Some(CrossAtlasSplitReason::Capacity);
    }
    if previous.pass_id != current.pass_id {
        return Some(CrossAtlasSplitReason::Pass);
    }
    if previous.clip_id != current.clip_id {
        return Some(CrossAtlasSplitReason::Clip);
    }
    if previous.material_id != current.material_id {
        return Some(CrossAtlasSplitReason::Material);
    }
    if previous.blend_mode != current.blend_mode || previous.filter_mode != current.filter_mode {
        return Some(CrossAtlasSplitReason::BlendFilter);
    }
    if !same_binding_domain(profile.binding, previous, current) {
        return Some(if profile.binding == CrossAtlasBinding::Conventional {
            CrossAtlasSplitReason::Texture
        } else {
            CrossAtlasSplitReason::BindingDomain
        });
    }
    None
}

fn baseline_counters(instances: &[CrossAtlasInstance]) -> CrossAtlasCounters {
    if instances.is_empty() {
        return CrossAtlasCounters::default();
    }
    let mut counters = CrossAtlasCounters {
        upload_bytes: (instances.len() * CROSS_ATLAS_INSTANCE_BYTES) as u64,
        upload_calls: instances.len() as u32,
        texture_binds: 1,
        draw_calls: instances.len() as u32,
        pass_changes: 0,
        queue_submissions: Some(1),
    };
    for pair in instances.windows(2) {
        if !same_binding_domain(CrossAtlasBinding::Conventional, &pair[0], &pair[1]) {
            counters.texture_binds += 1;
        }
        if pair[0].pass_id != pair[1].pass_id {
            counters.pass_changes += 1;
        }
    }
    counters
}

fn fallback_plan(
    profile: Option<CrossAtlasProfile>,
    instances: &[CrossAtlasInstance],
    reason: CrossAtlasFallbackReason,
) -> CrossAtlasPlan {
    let mut baseline = baseline_counters(instances);
    if profile.is_some_and(|value| value.queue_mode == CrossAtlasQueueMode::Unavailable) {
        baseline.queue_submissions = None;
    }
    CrossAtlasPlan {
        baseline,
        prototype: baseline,
        runs: Vec::new(),
        input_count: instances.len() as u32,
        order_hash: order_hash(instances),
        prototype_used: false,
        fallback_reason: reason,
    }
}

pub fn plan_cross_atlas_frame(
    profile: CrossAtlasProfile,
    instances: &[CrossAtlasInstance],
    run_capacity: usize,
    inject_upload_failure: bool,
) -> CrossAtlasPlan {
    if profile.max_instances_per_draw == 0 {
        return fallback_plan(
            Some(profile),
            instances,
            CrossAtlasFallbackReason::InvalidProfile,
        );
    }
    if !cross_atlas_instance_count_is_safe(instances.len()) {
        return fallback_plan(Some(profile), &[], CrossAtlasFallbackReason::SafeMaximum);
    }
    if instances
        .iter()
        .any(|instance| instance.feature_flags & !profile.supported_feature_flags != 0)
    {
        return fallback_plan(
            Some(profile),
            instances,
            CrossAtlasFallbackReason::UnsupportedFeature,
        );
    }
    if inject_upload_failure {
        return fallback_plan(
            Some(profile),
            instances,
            CrossAtlasFallbackReason::UploadFailure,
        );
    }

    let baseline = baseline_counters(instances);
    if instances.is_empty() {
        return CrossAtlasPlan {
            baseline,
            prototype: CrossAtlasCounters::default(),
            runs: Vec::new(),
            input_count: 0,
            order_hash: order_hash(instances),
            prototype_used: true,
            fallback_reason: CrossAtlasFallbackReason::None,
        };
    }

    let mut runs = vec![CrossAtlasRun {
        first_instance: 0,
        instance_count: 1,
        reason_before: CrossAtlasSplitReason::FrameStart,
    }];
    for (index, pair) in instances.windows(2).enumerate() {
        let current = runs.last_mut().expect("frame has an initial run");
        if let Some(reason) = split_reason(profile, &pair[0], &pair[1], current.instance_count) {
            runs.push(CrossAtlasRun {
                first_instance: (index + 1) as u32,
                instance_count: 1,
                reason_before: reason,
            });
        } else {
            current.instance_count += 1;
        }
    }
    if runs.len() > run_capacity {
        return fallback_plan(
            Some(profile),
            instances,
            CrossAtlasFallbackReason::OutputCapacity,
        );
    }

    let mut texture_binds = 1;
    for pair in instances.windows(2) {
        if !same_binding_domain(profile.binding, &pair[0], &pair[1]) {
            texture_binds += 1;
        }
    }
    let run_count = runs.len() as u32;
    let queue_submissions = match profile.queue_mode {
        CrossAtlasQueueMode::Unavailable => None,
        CrossAtlasQueueMode::One => Some(1),
        CrossAtlasQueueMode::PerRun => Some(run_count),
    };
    CrossAtlasPlan {
        baseline: CrossAtlasCounters {
            queue_submissions: if profile.queue_mode == CrossAtlasQueueMode::Unavailable {
                None
            } else {
                baseline.queue_submissions
            },
            ..baseline
        },
        prototype: CrossAtlasCounters {
            upload_bytes: (instances.len() * CROSS_ATLAS_INSTANCE_BYTES) as u64,
            upload_calls: if profile.one_frame_upload {
                1
            } else {
                run_count
            },
            texture_binds,
            draw_calls: run_count,
            pass_changes: baseline.pass_changes,
            queue_submissions,
        },
        runs,
        input_count: instances.len() as u32,
        order_hash: order_hash(instances),
        prototype_used: true,
        fallback_reason: CrossAtlasFallbackReason::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(binding: CrossAtlasBinding, capacity: u32) -> CrossAtlasProfile {
        CrossAtlasProfile {
            name: "test",
            binding,
            max_instances_per_draw: capacity,
            supported_feature_flags: 0x0f,
            one_frame_upload: true,
            queue_mode: CrossAtlasQueueMode::One,
        }
    }

    fn sprite(order: u32, resource_id: u32, binding_domain_id: u16) -> CrossAtlasInstance {
        CrossAtlasInstance {
            destination: [order as f32, 0.0, 16.0, 16.0],
            uv_crop: [0.0, 0.0, 1.0, 1.0],
            pivot: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
            tint_rgba: 0xffff_ffff,
            resource_id,
            order,
            clip_id: 0,
            binding_domain_id,
            material_id: 0,
            blend_mode: 0,
            filter_mode: 0,
            pass_id: 0,
            flags: 0,
            feature_flags: 0,
        }
    }

    #[test]
    fn layout_is_exact_and_fields_survive_planning() {
        assert_eq!(size_of::<CrossAtlasInstance>(), 80);
        assert_eq!(std::mem::offset_of!(CrossAtlasInstance, tint_rgba), 52);
        assert_eq!(std::mem::offset_of!(CrossAtlasInstance, resource_id), 56);
        assert_eq!(std::mem::offset_of!(CrossAtlasInstance, clip_id), 64);
        assert_eq!(
            std::mem::offset_of!(CrossAtlasInstance, binding_domain_id),
            68
        );
        assert_eq!(std::mem::offset_of!(CrossAtlasInstance, feature_flags), 76);
        let mut instances = [sprite(20, 1, 7), sprite(10, 2, 7), sprite(30, 1, 7)];
        instances[0].tint_rgba = 0x40ff_ffff;
        instances[1].destination = [19.25, -3.5, 16.0, 16.0];
        instances[1].uv_crop = [0.125, 0.25, 0.5, 0.75];
        instances[1].pivot = [4.0, 8.0];
        instances[1].scale = [-2.0, 3.0];
        instances[1].rotation = 0.75;
        instances[1].tint_rgba = 0x7f33_66cc;
        instances[1].flags = 3;
        let before = instances;
        let plan = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 32),
            &instances,
            3,
            false,
        );
        assert_eq!(instances, before);
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].instance_count, 3);
        assert_eq!(plan.order_hash, 0x1989_08e7);
        assert_eq!(plan.prototype.upload_bytes, 240);
        assert_eq!(plan.prototype.draw_calls, 1);
    }

    #[test]
    fn state_and_capacity_splits_are_deterministic() {
        let mut instances = std::array::from_fn::<_, 7, _>(|index| sprite(index as u32, 1, 7));
        instances[1].clip_id = 2;
        instances[2].clip_id = 2;
        instances[2].material_id = 3;
        instances[3] = instances[2];
        instances[3].order = 3;
        instances[3].blend_mode = 1;
        instances[4] = instances[3];
        instances[4].order = 4;
        instances[4].pass_id = 1;
        instances[5] = instances[4];
        instances[5].order = 5;
        instances[6] = instances[5];
        instances[6].order = 6;
        let plan = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 2),
            &instances,
            7,
            false,
        );
        assert_eq!(plan.runs.len(), 6);
        assert_eq!(plan.runs[1].reason_before, CrossAtlasSplitReason::Clip);
        assert_eq!(plan.runs[2].reason_before, CrossAtlasSplitReason::Material);
        assert_eq!(
            plan.runs[3].reason_before,
            CrossAtlasSplitReason::BlendFilter
        );
        assert_eq!(plan.runs[4].reason_before, CrossAtlasSplitReason::Pass);
        assert_eq!(plan.runs[5].reason_before, CrossAtlasSplitReason::Capacity);
        assert_eq!(plan.prototype.pass_changes, 1);
    }

    #[test]
    fn realized_binding_domains_are_not_compiler_groups() {
        let mut instances = [
            sprite(0, 10, 7),
            sprite(1, 11, 7),
            sprite(2, 10, 7),
            sprite(3, 11, 7),
        ];
        let conventional = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Conventional, 16),
            &instances,
            4,
            false,
        );
        assert_eq!(conventional.runs.len(), 4);
        assert_eq!(
            conventional.runs[1].reason_before,
            CrossAtlasSplitReason::Texture
        );

        for binding in [
            CrossAtlasBinding::TextureArray,
            CrossAtlasBinding::MegaAtlas,
        ] {
            let one_domain = plan_cross_atlas_frame(profile(binding, 16), &instances, 4, false);
            assert_eq!(one_domain.runs.len(), 1);
            assert_eq!(one_domain.prototype.texture_binds, 1);
            instances[2].binding_domain_id = 8;
            instances[3].binding_domain_id = 8;
            let two_domains = plan_cross_atlas_frame(profile(binding, 16), &instances, 4, false);
            assert_eq!(two_domains.runs.len(), 2);
            assert_eq!(
                two_domains.runs[1].reason_before,
                CrossAtlasSplitReason::BindingDomain
            );
            assert_eq!(two_domains.prototype.texture_binds, 2);
            instances[2].binding_domain_id = 7;
            instances[3].binding_domain_id = 7;
        }
        instances[2].binding_domain_id = 8;
        let bindless = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 16),
            &instances,
            4,
            false,
        );
        assert_eq!(bindless.runs.len(), 1);
        assert_eq!(bindless.prototype.texture_binds, 1);
    }

    #[test]
    fn failures_fall_back_before_exposing_runs() {
        let mut instances = [sprite(0, 1, 7), sprite(1, 2, 7)];
        instances[1].feature_flags = 0x80;
        let unsupported = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 16),
            &instances,
            2,
            false,
        );
        assert!(!unsupported.prototype_used);
        assert!(unsupported.runs.is_empty());
        assert_eq!(unsupported.prototype, unsupported.baseline);
        assert_eq!(
            unsupported.fallback_reason,
            CrossAtlasFallbackReason::UnsupportedFeature
        );
        instances[1].feature_flags = 0;
        let upload = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 16),
            &instances,
            2,
            true,
        );
        assert_eq!(
            upload.fallback_reason,
            CrossAtlasFallbackReason::UploadFailure
        );
        assert!(upload.runs.is_empty());
        let output = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 16),
            &instances,
            0,
            false,
        );
        assert_eq!(
            output.fallback_reason,
            CrossAtlasFallbackReason::OutputCapacity
        );
        assert!(output.runs.is_empty());
    }

    #[test]
    fn safe_maximum_and_invalid_profile_are_explicit() {
        assert!(cross_atlas_instance_count_is_safe(
            CROSS_ATLAS_SAFE_MAX_INSTANCES
        ));
        assert!(!cross_atlas_instance_count_is_safe(
            CROSS_ATLAS_SAFE_MAX_INSTANCES + 1
        ));
        let plan = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 0),
            &[sprite(0, 1, 7)],
            1,
            false,
        );
        assert_eq!(
            plan.fallback_reason,
            CrossAtlasFallbackReason::InvalidProfile
        );
    }

    #[test]
    fn planner_contract_is_owned_handle_free_and_cross_host_safe() {
        fn assert_portable_value<T: Copy + Send + Sync + 'static>() {}
        assert_portable_value::<CrossAtlasInstance>();
        assert_portable_value::<CrossAtlasProfile>();
        assert_portable_value::<CrossAtlasRun>();
        assert_portable_value::<CrossAtlasCounters>();

        let instance = sprite(0, 42, 3);
        let plan = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::TextureArray, 64),
            std::slice::from_ref(&instance),
            1,
            false,
        );
        assert!(plan.prototype_used);
        assert_eq!(plan.runs[0].first_instance, 0);
        assert_eq!(plan.prototype.upload_bytes, 80);
    }

    #[test]
    fn interleaving_contiguity_and_domain_spill_have_exact_costs() {
        let interleaved = (0..4_096)
            .map(|index| sprite(index, index % 8, 1))
            .collect::<Vec<_>>();
        let asset_major = (0..4_096)
            .map(|index| sprite(index, index / 512, 1))
            .collect::<Vec<_>>();
        let spilled = (0..4_096)
            .map(|index| {
                let resource = index % 8;
                sprite(index, resource, if resource < 4 { 1 } else { 2 })
            })
            .collect::<Vec<_>>();

        for binding in [
            CrossAtlasBinding::MegaAtlas,
            CrossAtlasBinding::TextureArray,
        ] {
            let interleaved_plan =
                plan_cross_atlas_frame(profile(binding, 4_096), &interleaved, 4_096, false);
            assert_eq!(interleaved_plan.baseline.texture_binds, 4_096);
            assert_eq!(interleaved_plan.baseline.draw_calls, 4_096);
            assert_eq!(interleaved_plan.prototype.texture_binds, 1);
            assert_eq!(interleaved_plan.prototype.draw_calls, 1);

            let asset_major_plan =
                plan_cross_atlas_frame(profile(binding, 4_096), &asset_major, 4_096, false);
            assert_eq!(asset_major_plan.baseline.texture_binds, 8);
            assert_eq!(asset_major_plan.prototype.texture_binds, 1);
            assert_eq!(asset_major_plan.prototype.draw_calls, 1);

            let spilled_plan =
                plan_cross_atlas_frame(profile(binding, 4_096), &spilled, 4_096, false);
            assert_eq!(spilled_plan.baseline.texture_binds, 4_096);
            assert_eq!(spilled_plan.prototype.texture_binds, 1_024);
            assert_eq!(spilled_plan.prototype.draw_calls, 1_024);
        }

        let bindless = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Bindless, 4_096),
            &spilled,
            4_096,
            false,
        );
        assert_eq!(bindless.prototype.texture_binds, 1);
        assert_eq!(bindless.prototype.draw_calls, 1);

        let conventional = plan_cross_atlas_frame(
            profile(CrossAtlasBinding::Conventional, 4_096),
            &asset_major,
            4_096,
            false,
        );
        assert_eq!(conventional.prototype.texture_binds, 8);
        assert_eq!(conventional.prototype.draw_calls, 8);
    }
}
