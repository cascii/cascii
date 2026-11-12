mod gpu;
use anyhow::{ensure, Context, Result};
use cascii::{image_to_frame, ConversionOptions};
use image::{DynamicImage, RgbImage};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

fn cpu(img: &DynamicImage, opts: &ConversionOptions) -> Vec<u8> {
    image_to_frame(img, opts).unwrap().cframe_bytes()
}
fn stats(v: &[f64]) -> Value {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    json!({"median_ms":s[s.len()/2],"min_ms":s[0],"max_ms":s[s.len()-1],"samples_ms":v})
}
fn quality(a: &[u8], b: &[u8]) -> Value {
    assert_eq!(a.len(), b.len());
    assert_eq!(&a[..8], &b[..8]);
    let mut mismatches = 0;
    let mut max = 0;
    let mut sum = 0u64;
    let mut sq = 0u64;
    for (a, b) in a[8..].chunks_exact(4).zip(b[8..].chunks_exact(4)) {
        mismatches += usize::from(a[0] != b[0]);
        for k in 1..4 {
            let d = a[k].abs_diff(b[k]);
            max = max.max(d);
            sum += u64::from(d);
            sq += u64::from(d) * u64::from(d);
        }
    }
    let cells = (a.len() - 8) / 4;
    json!({"cells":cells,"char_mismatches":mismatches,"char_mismatch_pct":100.0*mismatches as f64/cells as f64,"rgb_max_abs":max,"rgb_mae":sum as f64/(cells*3) as f64,"rgb_mse":sq as f64/(cells*3) as f64,"byte_identical":a==b})
}
fn generated(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| image::Rgb([((x * 13 + y * 3) % 256) as u8, ((x * 3 + y * 7) % 256) as u8, ((x ^ y) % 256) as u8])))
}
fn frames() -> Result<()> {
    let start = Instant::now();
    let gpu = gpu::Gpu::new()?;
    let init = start.elapsed().as_secs_f64() * 1000.;
    let mut cases = vec![("synthetic_360p".to_string(), generated(640, 360)), ("synthetic_1080p".to_string(), generated(1920, 1080)), ("synthetic_4k".to_string(), generated(3840, 2160))];
    for name in ["source.png", "test_01.png", "test_02.png", "test_03.png", "test_04.png"] {
        cases.push((name.into(), image::open(Path::new("resources").join(name))?));
    }
    let mut results = vec![];
    for (name, img) in cases {
        for columns in [80, 400, 1000] {
            let opts = ConversionOptions::default().with_columns(columns);
            let rgb = img.to_rgb8();
            let dh = (img.height() as f32 / img.width() as f32 * columns as f32 * opts.font_ratio).round().max(1.) as u32;
            let start = Instant::now();
            let plan = gpu.plan(img.width(), img.height(), columns, dh, opts.luminance, opts.ascii_chars.as_bytes())?;
            let plan_ms = start.elapsed().as_secs_f64() * 1000.;
            let expected = cpu(&img, &opts);
            let actual = gpu.convert(&plan, rgb.as_raw())?;
            let q = quality(&expected, &actual);
            ensure!(q["rgb_max_abs"].as_u64().unwrap() <= 1, "RGB difference exceeds tolerance");
            ensure!(q["char_mismatch_pct"].as_f64().unwrap() <= 0.2, "character difference exceeds exploratory tolerance");
            let mut ct = vec![];
            let mut gt = vec![];
            for _ in 0..3 {
                std::hint::black_box(cpu(&img, &opts));
                std::hint::black_box(gpu.convert(&plan, rgb.as_raw())?);
            }
            for i in 0..15 {
                // Alternate order to reduce systematic thermal/order bias.
                for backend in if i % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let t = Instant::now();
                    if backend == 0 {
                        std::hint::black_box(cpu(&img, &opts));
                        ct.push(t.elapsed().as_secs_f64() * 1000.);
                    } else {
                        std::hint::black_box(gpu.convert(&plan, rgb.as_raw())?);
                        gt.push(t.elapsed().as_secs_f64() * 1000.);
                    }
                }
            }
            eprintln!("frame {name} {columns}: CPU {} GPU {} ms", stats(&ct)["median_ms"], stats(&gt)["median_ms"]);
            results.push(json!({"case":name,"width":img.width(),"height":img.height(),"columns":columns,"rows":dh,"plan_ms":plan_ms,"cpu":stats(&ct),"gpu":stats(&gt),"quality":q}));
        }
    }
    // Edge cases: identity, upscale, odd dimensions, tiny images, ramp/threshold boundaries.
    let mut edges = vec![];
    for (sw, sh, dw, dh) in [(1, 1, 1, 1), (37, 29, 37, 29), (37, 29, 11, 7), (13, 9, 41, 33)] {
        for threshold in [0, 20, 254, 255] {
            let img = generated(sw, sh);
            let opts = ConversionOptions::default().with_columns(dw).with_font_ratio(dh as f32 * sw as f32 / (sh as f32 * dw as f32)).with_luminance(threshold);
            let plan = gpu.plan(sw, sh, dw, dh, threshold, opts.ascii_chars.as_bytes())?;
            let q = quality(&cpu(&img, &opts), &gpu.convert(&plan, img.to_rgb8().as_raw())?);
            ensure!(q["rgb_max_abs"].as_u64().unwrap() <= 1, "edge error");
            edges.push(json!({"dims":[sw,sh,dw,dh],"threshold":threshold,"quality":q}));
        }
    }
    let gray = DynamicImage::ImageRgb8(RgbImage::from_fn(256, 1, |x, _| image::Rgb([x as u8; 3])));
    for threshold in [0, 20, 254, 255] {
        for ramp in [" @", "X", ".:-=+#"] {
            let opts = ConversionOptions::default().with_columns(256).with_font_ratio(1.).with_luminance(threshold).with_ascii_chars(ramp.into());
            let plan = gpu.plan(256, 1, 256, 1, threshold, ramp.as_bytes())?;
            let q = quality(&cpu(&gray, &opts), &gpu.convert(&plan, gray.to_rgb8().as_raw())?);
            ensure!(q["byte_identical"] == true, "identity threshold/ramp boundary mismatch");
            edges.push(json!({"case":"all_256_gray_levels","threshold":threshold,"ramp":ramp,"quality":q}));
        }
    }
    ensure!(gpu.plan(0, 1, 1, 1, 20, b"X").is_err(), "zero width accepted");
    ensure!(gpu.plan(1, 1, 1, 1, 20, b"").is_err(), "empty ramp accepted");
    println!("{}", serde_json::to_string_pretty(&json!({"adapter":gpu.adapter,"init_ms":init,"results":results,"edge_tests":edges}))?);
    Ok(())
}
fn video(args: &[String]) -> Result<()> {
    // video cpu|gpu input sw sh columns decode(cpu|cuda) prescale(0|1) output_dir
    let backend = &args[2];
    let input = &args[3];
    let sw: u32 = args[4].parse()?;
    let sh: u32 = args[5].parse()?;
    let cols: u32 = args[6].parse()?;
    let hw = &args[7] == "cuda";
    let prescale = &args[8] == "1";
    let out = Path::new(&args[9]);
    std::fs::create_dir_all(out)?;
    let total = Instant::now();
    let gpu = if backend == "gpu" { Some(gpu::Gpu::new()?) } else { None };
    let w = if prescale { cols } else { sw };
    // FFmpeg scale=-2 rounds to nearest multiple of two.
    let h = if prescale { ((sh as f64 / sw as f64 * cols as f64 / 2.).round() as u32) * 2 } else { sh };
    let opts = ConversionOptions::default().with_columns(cols);
    let dh = (h as f32 / w as f32 * cols as f32 * opts.font_ratio).round().max(1.) as u32;
    let plan = if let Some(g) = &gpu { Some(g.plan(w, h, cols, dh, opts.luminance, opts.ascii_chars.as_bytes())?) } else { None };
    let setup_ms = total.elapsed().as_secs_f64() * 1000.;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if hw {
        cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
    }
    cmd.args(["-i", input]);
    let filter = match (hw, prescale) {
        (false, false) => "fps=30".into(),
        (false, true) => format!("scale={cols}:-2,fps=30"),
        (true, false) => "hwdownload,format=nv12,fps=30".into(),
        (true, true) => format!("scale_cuda={cols}:{h}:interp_algo=bilinear,hwdownload,format=nv12,fps=30"),
    };
    cmd.args(["-vf", &filter, "-an", "-sn", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()?;
    let mut stream = child.stdout.take().context("stdout")?;
    let mut rgb = vec![0u8; w as usize * h as usize * 3];
    let mut count = 0;
    let mut compute_ms = 0.;
    let mut io_ms = 0.;
    loop {
        let mut filled = 0;
        while filled < rgb.len() {
            let n = stream.read(&mut rgb[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        ensure!(filled == rgb.len(), "partial raw frame");
        let t = Instant::now();
        let bytes = if let (Some(g), Some(p)) = (&gpu, &plan) { g.convert(p, &rgb)? } else { cpu(&DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, rgb.clone()).unwrap()), &opts) };
        compute_ms += t.elapsed().as_secs_f64() * 1000.;
        let t = Instant::now();
        std::fs::write(out.join(format!("frame_{:04}.cframe", count + 1)), bytes)?;
        io_ms += t.elapsed().as_secs_f64() * 1000.;
        count += 1;
    }
    ensure!(child.wait()?.success(), "ffmpeg failed");
    ensure!(count > 0, "no frames");
    println!("{}", json!({"backend":backend,"hwdecode":hw,"prescale":prescale,"frames":count,"setup_ms":setup_ms,"compute_ms":compute_ms,"write_ms":io_ms,"total_ms":total.elapsed().as_secs_f64()*1000.}));
    Ok(())
}
fn single_image(args: &[String]) -> Result<()> {
    ensure!(args.len() == 5, "image cpu|gpu input output.cframe");
    let t = Instant::now();
    let opts = ConversionOptions::default();
    let img = image::open(&args[3])?;
    let bytes = if args[2] == "gpu" {
        let gpu = gpu::Gpu::new()?;
        let rgb = img.to_rgb8();
        let cols = opts.columns.unwrap();
        let rows = (img.height() as f32 / img.width() as f32 * cols as f32 * opts.font_ratio).round().max(1.) as u32;
        let plan = gpu.plan(img.width(), img.height(), cols, rows, opts.luminance, opts.ascii_chars.as_bytes())?;
        gpu.convert(&plan, rgb.as_raw())?
    } else {
        cpu(&img, &opts)
    };
    std::fs::write(&args[4], bytes)?;
    println!("{}", json!({"total_ms":t.elapsed().as_secs_f64()*1000.}));
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("frames") => frames(),
        Some("video") => {
            ensure!(args.len() == 10, "video cpu|gpu input width height columns cpu|cuda 0|1 output_dir");
            video(&args)
        }
        Some("image") => single_image(&args),
        _ => anyhow::bail!("Use frames, image cpu|gpu input output.cframe, or video cpu|gpu input width height columns cpu|cuda 0|1 output_dir"),
    }
}
