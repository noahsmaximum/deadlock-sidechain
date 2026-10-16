//! Stand-in for a game's GPU load, to measure neural stems under contention:
//! `gpuload [iterations] [seconds]` renders a heavy full-window shader uncapped (off-screen window)
//! and prints fps once a second.
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use eframe::glow::{self, HasContext};

const VERT: &str = "#version 330\nconst vec2 P[3] = vec2[3](vec2(-1,-1), vec2(3,-1), vec2(-1,3));\nvoid main(){ gl_Position = vec4(P[gl_VertexID], 0, 1); }";
const FRAG: &str = "#version 330\nuniform int n; uniform float t; out vec4 o;\nvoid main(){ vec2 p = gl_FragCoord.xy * 0.001; float a = t;\n for (int i = 0; i < n; i++) { p = vec2(sin(p.y * 1.3 + a), cos(p.x * 1.7 - a)) + p * 0.5; a += 0.01; }\n o = vec4(p, 0.5, 1); }";

struct Load {
    prog: glow::Program,
    vao: glow::VertexArray,
}

struct App {
    load: Arc<Load>,
    iters: i32,
    start: Instant,
    frames: u32,
    last: Instant,
    until: f32,
    prev: Instant,
    worst: f32,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let (load, n, t) = (self.load.clone(), self.iters, self.start.elapsed().as_secs_f32());
        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.max_rect();
            ui.painter().add(egui::PaintCallback {
                rect,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, p| unsafe {
                    let gl = p.gl();
                    gl.use_program(Some(load.prog));
                    gl.uniform_1_i32(gl.get_uniform_location(load.prog, "n").as_ref(), n);
                    gl.uniform_1_f32(gl.get_uniform_location(load.prog, "t").as_ref(), t);
                    gl.bind_vertex_array(Some(load.vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, 3);
                })),
            });
        });
        self.frames += 1;
        self.worst = self.worst.max(self.prev.elapsed().as_secs_f32() * 1000.0);
        self.prev = Instant::now();
        if self.last.elapsed().as_secs_f32() >= 1.0 {
            println!("{} fps  worst frame {:.1} ms", self.frames, self.worst);
            self.frames = 0;
            self.worst = 0.0;
            self.last = Instant::now();
        }
        if self.until > 0.0 && t > self.until {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let iters = args.first().and_then(|s| s.parse().ok()).unwrap_or(200);
    let until = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1920.0, 1080.0])
            .with_position([-4000.0, -4000.0])
            .with_active(false)
            .with_title("gpuload"),
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "gpuload",
        opts,
        Box::new(move |cc| {
            let gl = cc.gl.as_ref().expect("glow");
            let load = unsafe {
                let prog = gl.create_program().unwrap();
                for (ty, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, FRAG)] {
                    let sh = gl.create_shader(ty).unwrap();
                    gl.shader_source(sh, src);
                    gl.compile_shader(sh);
                    assert!(gl.get_shader_compile_status(sh), "{}", gl.get_shader_info_log(sh));
                    gl.attach_shader(prog, sh);
                }
                gl.link_program(prog);
                assert!(gl.get_program_link_status(prog), "{}", gl.get_program_info_log(prog));
                Load { prog, vao: gl.create_vertex_array().unwrap() }
            };
            Ok(Box::new(App { load: Arc::new(load), iters, start: Instant::now(), frames: 0, last: Instant::now(), until, prev: Instant::now(), worst: 0.0 }))
        }),
    )
}
