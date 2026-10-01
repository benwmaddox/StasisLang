use super::*;
use crate::backend::jit::JitProcess;
use object::{Architecture, BinaryFormat, File, Object, ObjectSection, SectionKind};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
enum Shape {
    Position,
    WallSparse,
    WallDense,
    BrickoutActive,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::WallSparse => "wall_sparse",
            Self::WallDense => "wall_dense",
            Self::BrickoutActive => "brickout_active",
        }
    }
}

fn active_count(shape: Shape, capacity: usize) -> usize {
    match shape {
        Shape::BrickoutActive if capacity < 900 => capacity / 2,
        Shape::BrickoutActive => 90,
        Shape::Position | Shape::WallSparse | Shape::WallDense => capacity,
    }
}

fn active_stride(shape: Shape, capacity: usize) -> usize {
    match shape {
        Shape::BrickoutActive if capacity < 900 => 2,
        Shape::BrickoutActive => 10,
        Shape::Position | Shape::WallSparse | Shape::WallDense => 1,
    }
}

fn fixture(shape: Shape, capacity: usize) -> String {
    let active = active_count(shape, capacity);
    let active_stride = active_stride(shape, capacity);
    let (initialize_special, tick_body, reference_body) = match shape {
        Shape::Position => (
            "",
            r#"
        ball.x = ball.x + ball.vx;
        ball.y = ball.y + ball.vy;
"#,
            r#"
        ref_x[i] = ref_x[i] + ref_vx[i];
        ref_y[i] = ref_y[i] + ref_vy[i];
"#,
        ),
        Shape::WallSparse => (
            r#"
        if (i == 0) {
            balls[i].x = 99.0;
            balls[i].vx = 2.0;
            ref_x[i] = 99.0;
            ref_vx[i] = 2.0;
        }
"#,
            r#"
        ball.x = ball.x + ball.vx;
        ball.y = ball.y + ball.vy;
        if (ball.x <= 0.0 || ball.x >= 100.0) { ball.vx = 0.0 - ball.vx; }
        if (ball.y <= 0.0 || ball.y >= 80.0) { ball.vy = 0.0 - ball.vy; }
"#,
            r#"
        ref_x[i] = ref_x[i] + ref_vx[i];
        ref_y[i] = ref_y[i] + ref_vy[i];
        if (ref_x[i] <= 0.0 || ref_x[i] >= 100.0) { ref_vx[i] = 0.0 - ref_vx[i]; }
        if (ref_y[i] <= 0.0 || ref_y[i] >= 80.0) { ref_vy[i] = 0.0 - ref_vy[i]; }
"#,
        ),
        Shape::WallDense => (
            r#"
        if (i % 2 == 0) {
            balls[i].x = 99.0;
            balls[i].vx = 2.0;
            ref_x[i] = 99.0;
            ref_vx[i] = 2.0;
        } else {
            balls[i].x = 1.0;
            balls[i].vx = 0.0 - 2.0;
            ref_x[i] = 1.0;
            ref_vx[i] = 0.0 - 2.0;
        }
        balls[i].y = 79.0;
        balls[i].vy = 2.0;
        ref_y[i] = 79.0;
        ref_vy[i] = 2.0;
"#,
            r#"
        ball.x = ball.x + ball.vx;
        ball.y = ball.y + ball.vy;
        if (ball.x <= 0.0 || ball.x >= 100.0) { ball.vx = 0.0 - ball.vx; }
        if (ball.y <= 0.0 || ball.y >= 80.0) { ball.vy = 0.0 - ball.vy; }
"#,
            r#"
        ref_x[i] = ref_x[i] + ref_vx[i];
        ref_y[i] = ref_y[i] + ref_vy[i];
        if (ref_x[i] <= 0.0 || ref_x[i] >= 100.0) { ref_vx[i] = 0.0 - ref_vx[i]; }
        if (ref_y[i] <= 0.0 || ref_y[i] >= 80.0) { ref_vy[i] = 0.0 - ref_vy[i]; }
"#,
        ),
        Shape::BrickoutActive => (
            r#"
        balls[i].active = i % ACTIVE_STRIDE == 0;
        ref_active[i] = i % ACTIVE_STRIDE == 0;
        if (i % 8 == 0) {
            balls[i].x = 0.5;
            balls[i].vx = 0.0 - 2.0;
            ref_x[i] = 0.5;
            ref_vx[i] = 0.0 - 2.0;
        }
        if (i % 8 == 1) {
            balls[i].x = 99.5;
            balls[i].vx = 2.0;
            ref_x[i] = 99.5;
            ref_vx[i] = 2.0;
        }
        if (i % 8 == 2) {
            balls[i].y = 79.5;
            balls[i].vy = 2.0;
            ref_y[i] = 79.5;
            ref_vy[i] = 2.0;
        }
"#,
            r#"
        if (ball.active) {
            ball.x = ball.x + ball.vx * 0.25;
            ball.y = ball.y + ball.vy * 0.25;
            let bounced: bool = false;
            if (ball.x < ball.radius) {
                ball.x = ball.radius;
                ball.vx = abs_value(ball.vx);
                bounced = true;
            }
            if (ball.x > 100.0 - ball.radius) {
                ball.x = 100.0 - ball.radius;
                ball.vx = 0.0 - abs_value(ball.vx);
                bounced = true;
            }
            if (ball.y > 80.0 - ball.radius) {
                ball.y = 80.0 - ball.radius;
                ball.vy = 0.0 - abs_value(ball.vy);
                bounced = true;
            }
            if (bounced) { ball.bounce_streak = ball.bounce_streak + 1; }
            if (ball.y < 0.0 - ball.radius) { ball.active = false; }
        }
"#,
            r#"
        if (ref_active[i]) {
            ref_x[i] = ref_x[i] + ref_vx[i] * 0.25;
            ref_y[i] = ref_y[i] + ref_vy[i] * 0.25;
            let bounced: bool = false;
            if (ref_x[i] < ref_radius[i]) {
                ref_x[i] = ref_radius[i];
                ref_vx[i] = abs_value(ref_vx[i]);
                bounced = true;
            }
            if (ref_x[i] > 100.0 - ref_radius[i]) {
                ref_x[i] = 100.0 - ref_radius[i];
                ref_vx[i] = 0.0 - abs_value(ref_vx[i]);
                bounced = true;
            }
            if (ref_y[i] > 80.0 - ref_radius[i]) {
                ref_y[i] = 80.0 - ref_radius[i];
                ref_vy[i] = 0.0 - abs_value(ref_vy[i]);
                bounced = true;
            }
            if (bounced) { ref_bounce_streak[i] = ref_bounce_streak[i] + 1; }
            if (ref_y[i] < 0.0 - ref_radius[i]) { ref_active[i] = false; }
        }
"#,
        ),
    };

    format!(
        r#"
const CAPACITY: i32 = {capacity};
const ACTIVE_COUNT: i32 = {active};
const ACTIVE_STRIDE: i32 = {active_stride};
struct Ball {{
    active: bool;
    x: f32;
    y: f32;
    vx: f32;
    vy: f32;
    radius: f32;
    bounce_streak: i32;
}}
global balls: Ball[CAPACITY];
global ref_active: bool[CAPACITY];
global ref_x: f32[CAPACITY];
global ref_y: f32[CAPACITY];
global ref_vx: f32[CAPACITY];
global ref_vy: f32[CAPACITY];
global ref_radius: f32[CAPACITY];
global ref_bounce_streak: i32[CAPACITY];

function abs_value(value: f32): f32 {{
    if (value < 0.0) {{ return 0.0 - value; }}
    return value;
}}

function tick(): void {{
    foreach (let ball in balls) {{{tick_body}
    }}
    return;
}}

function reference_tick(): void {{
    let i: i32 = 0;
    for (i = 0; i < CAPACITY; i = i + 1) {{{reference_body}
    }}
    return;
}}

function verify_state(): i32 {{
    let i: i32 = 0;
    for (i = 0; i < CAPACITY; i = i + 1) {{
        if (balls[i].active != ref_active[i]) {{ return 1; }}
        if (balls[i].x != ref_x[i]) {{ return 2; }}
        if (balls[i].y != ref_y[i]) {{ return 3; }}
        if (balls[i].vx != ref_vx[i]) {{ return 4; }}
        if (balls[i].vy != ref_vy[i]) {{ return 5; }}
        if (balls[i].radius != ref_radius[i]) {{ return 6; }}
        if (balls[i].bounce_streak != ref_bounce_streak[i]) {{ return 7; }}
    }}
    return 0;
}}

function initialize(): void {{
    let i: i32 = 0;
    for (i = 0; i < CAPACITY; i = i + 1) {{
        let fi: f32 = 0.0;
        fi.from_i32(i);
        balls[i].active = true;
        balls[i].x = 20.0 + fi * 0.01;
        balls[i].y = 30.0 + fi * 0.01;
        balls[i].vx = 1.0;
        balls[i].vy = 0.0 - 2.0;
        balls[i].radius = 1.0;
        balls[i].bounce_streak = 0;
        ref_active[i] = balls[i].active;
        ref_x[i] = balls[i].x;
        ref_y[i] = balls[i].y;
        ref_vx[i] = balls[i].vx;
        ref_vy[i] = balls[i].vy;
        ref_radius[i] = balls[i].radius;
        ref_bounce_streak[i] = balls[i].bounce_streak;
{initialize_special}
    }}
    return;
}}

function main(): i32 {{
    initialize();
    let iteration: i32 = 0;
    for (iteration = 0; iteration < 5; iteration = iteration + 1) {{
        tick();
        reference_tick();
        let mismatch: i32 = verify_state();
        if (mismatch != 0) {{ return mismatch; }}
    }}
    return 0;
}}
"#
    )
}

fn configured_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn compile_arm64(source: &str) -> AotProcess {
    let mut process = AotProcess::with_optimization_profile(AotOptimizationProfile::SpeedAndSize);
    process.set_target(stasis_jit::AotTarget::android_arm64_default());
    process.set_required_emit_roots(&["main".to_string(), "tick".to_string()]);
    process.upsert_file("simd_soa_characterization.stasis", source);
    process.compile().expect("compile Android ARM64 fixture");
    process
}

fn capture_aot_clif_by_function(process: &mut AotProcess) -> BTreeMap<String, String> {
    let _capture_lock = CLIF_CAPTURE_LOCK.lock().expect("lock CLIF capture");
    let captured: Arc<Mutex<BTreeMap<String, String>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let captured_hook = Arc::clone(&captured);
    set_clif_dump_hook(Some(Box::new(move |meta, func| {
        captured_hook
            .lock()
            .expect("lock clif capture")
            .insert(meta.name.clone(), format!("{}", func.display()));
    })));
    let compile_result = process.compile();
    set_clif_dump_hook(None);
    compile_result.expect("aot compile");
    let captured = captured.lock().expect("lock clif capture").clone();
    captured
}

fn text_size(path: &Path) -> u64 {
    let bytes = fs::read(path).expect("read emitted object");
    let file = File::parse(bytes.as_slice()).expect("parse emitted object");
    assert_eq!(file.format(), BinaryFormat::Elf);
    assert_eq!(file.architecture(), Architecture::Aarch64);
    file.sections()
        .filter(|section| section.kind() == SectionKind::Text)
        .map(|section| section.size())
        .sum()
}

fn scalar_clif_counts(clif: &str) -> Value {
    let count = |needle: &str| clif.lines().filter(|line| line.contains(needle)).count();
    let vector_type_markers: Vec<&str> = [
        "f32x4", "f32x8", "f64x2", "f64x4", "i8x16", "i16x8", "i32x4", "i64x2",
    ]
    .into_iter()
    .filter(|marker| clif.contains(marker))
    .collect();
    json!({
        "branches": count("brif"),
        "f32_adds": count("fadd"),
        "f32_multiplies": count("fmul"),
        "loads": count("load."),
        "stores": count("store "),
        "vector_type_markers": vector_type_markers,
    })
}

#[test]
fn simd_soa_characterization_records_arm64_evidence() {
    let warmups = configured_count("STASIS_SIMD_COMPILE_WARMUPS", 0);
    let repetitions = configured_count("STASIS_SIMD_COMPILE_REPETITIONS", 1).max(1);
    let configured_output = std::env::var_os("STASIS_SIMD_EVIDENCE_DIR").map(PathBuf::from);
    let output_root = configured_output.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "stasis-simd-characterization-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    });
    fs::create_dir_all(&output_root).expect("create characterization output");

    let mut rows = Vec::new();
    for shape in [
        Shape::Position,
        Shape::WallSparse,
        Shape::WallDense,
        Shape::BrickoutActive,
    ] {
        for capacity in [4usize, 64, 900] {
            let label = format!("{}_{capacity}", shape.label());
            let source = fixture(shape, capacity);

            let mut jit = JitProcess::new();
            jit.set_required_emit_roots(&["main".to_string(), "tick".to_string()]);
            jit.upsert_file("simd_soa_characterization.stasis", &source);
            jit.compile().expect("compile semantic oracle");
            assert_eq!(
                jit.execute_i32_noarg_by_name("main")
                    .expect("execute semantic oracle"),
                0,
                "state mismatch for {label}"
            );
            let jit_clif = jit
                .clif_for_function_name("tick")
                .expect("tick JIT CLIF")
                .to_string();

            for _ in 0..warmups {
                let _ = compile_arm64(&source);
            }
            let mut compile_us = Vec::with_capacity(repetitions);
            for _ in 0..repetitions {
                let started = Instant::now();
                let _ = compile_arm64(&source);
                compile_us.push(started.elapsed().as_micros() as u64);
            }

            let mut aot =
                AotProcess::with_optimization_profile(AotOptimizationProfile::SpeedAndSize);
            aot.set_target(stasis_jit::AotTarget::android_arm64_default());
            aot.set_required_emit_roots(&["main".to_string(), "tick".to_string()]);
            aot.upsert_file("simd_soa_characterization.stasis", &source);
            let captured = capture_aot_clif_by_function(&mut aot);
            let aot_clif = captured.get("tick").expect("tick AOT CLIF");

            let case_dir = output_root.join(&label);
            fs::create_dir_all(&case_dir).expect("create case output");
            fs::write(case_dir.join("fixture.stasis"), &source).expect("write source fixture");
            fs::write(case_dir.join("tick.jit.clif"), &jit_clif).expect("write JIT CLIF");
            fs::write(case_dir.join("tick.aot.clif"), aot_clif).expect("write AOT CLIF");
            let objects = aot
                .write_object_files(&case_dir.join("objects"))
                .expect("write AOT objects");
            let (_, tick_path) = objects.get("tick").expect("tick object");
            let relative_tick = tick_path
                .strip_prefix(&output_root)
                .expect("tick object below output root")
                .to_string_lossy()
                .replace('\\', "/");
            rows.push(json!({
                "active_count": active_count(shape, capacity),
                "aot_clif": scalar_clif_counts(aot_clif),
                "aot_compile_total_us": compile_us,
                "capacity": capacity,
                "case": label,
                "jit_clif": scalar_clif_counts(&jit_clif),
                "semantic_ticks": 5,
                "shape": shape.label(),
                "tick_object": relative_tick,
                "tick_text_bytes": text_size(tick_path),
            }));
        }
    }

    let manifest = json!({
        "cases": rows,
        "compile_timing": {
            "measure": "whole AotProcess compile; frontend plus Cranelift codegen",
            "profile": "speed_and_size",
            "repetitions": repetitions,
            "warmups": warmups,
        },
        "schema": "stasis.simd_soa_characterization.v1",
        "target": "aarch64-linux-android",
    });
    fs::write(
        output_root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize manifest"),
    )
    .expect("write characterization manifest");

    if configured_output.is_none() {
        fs::remove_dir_all(&output_root).expect("remove temporary characterization output");
    } else {
        eprintln!("SIMD_SOA_EVIDENCE={}", output_root.display());
    }
}
