//! Time HT-Demucs models: `stembench <model.onnx> [dml|cpu] [runs] [threads]`.
//! Works for the full-graph export (input `mix`) and the forward-only one (inputs `x`, `xt`).
use std::time::Instant;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model = args.first().cloned().unwrap_or("models/htdemucs_fwd.onnx".into());
    let ep = args.get(1).cloned().unwrap_or("dml".into());
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);

    let env = ort::init().with_name("stembench").build()?;
    let mut b = Session::builder(&env)?.with_optimization_level(GraphOptimizationLevel::Level3).map_err(|e| anyhow::anyhow!("{e}"))?;
    if ep == "dml" {
        b = b
            .with_memory_pattern(false)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_parallel_execution(false)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    } else {
        b = b.with_intra_threads(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4)).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let t = Instant::now();
    let mut s = b.commit_from_file(&model)?;
    println!("load {:.2}s ({ep})", t.elapsed().as_secs_f32());
    for i in s.inputs() {
        println!("  in  {} {:?}", i.name(), i.dtype());
    }
    for o in s.outputs() {
        println!("  out {} {:?}", o.name(), o.dtype());
    }
    let names: Vec<String> = s.inputs().iter().map(|i| i.name().to_string()).collect();

    for r in 0..runs + 1 {
        let t = Instant::now();
        let out_len;
        if names.len() == 1 {
            let x = Tensor::from_array(([1usize, 2, 343_980], vec![0.01f32; 2 * 343_980].into_boxed_slice()))?;
            let out = s.run(ort::inputs![names[0].as_str() => x])?;
            out_len = out[0].try_extract_tensor::<f32>()?.1.len();
        } else {
            let x = Tensor::from_array(([1usize, 4, 2048, 336], vec![0.01f32; 4 * 2048 * 336].into_boxed_slice()))?;
            let xt = Tensor::from_array(([1usize, 2, 343_980], vec![0.01f32; 2 * 343_980].into_boxed_slice()))?;
            let out = s.run(ort::inputs!["x" => x, "xt" => xt])?;
            out_len = out[0].try_extract_tensor::<f32>()?.1.len() + out[1].try_extract_tensor::<f32>()?.1.len();
        }
        let dt = t.elapsed().as_secs_f32();
        let label = if r == 0 { "warm-up".to_string() } else { format!("run {r}") };
        println!("{label} {dt:.3}s  (RTF {:.3}, {out_len} out values)", dt / 7.8);
    }
    Ok(())
}
