//! One-shot MF device dump for diagnostics.
//! Run: cargo run -p capture --example list_mf

fn main() {
    match capture::list_devices() {
        Ok(devs) => {
            println!("count={}", devs.len());
            for (i, d) in devs.iter().enumerate() {
                println!(
                    "[{i}] backend={} score={} virtual={} name={:?} id={} formats={} note={:?}",
                    d.backend,
                    capture::device_capture_score_for(&d.display_name, d.id.as_str()),
                    capture::is_virtual_camera(&d.display_name, d.id.as_str()),
                    d.display_name,
                    d.id.as_str(),
                    d.formats.len(),
                    d.enum_note
                );
                for f in d.formats.iter().take(8) {
                    let rate = f
                        .frame_rates
                        .first()
                        .map(|r| format!("{:.2}", r.as_f64()))
                        .unwrap_or_else(|| "?".into());
                    println!(
                        "    {:?} {}x{} @ {} interlaced={}",
                        f.pixel_format, f.width, f.height, rate, f.interlaced
                    );
                }
                if d.formats.len() > 8 {
                    println!("    ... +{} more", d.formats.len() - 8);
                }
            }
        }
        Err(e) => {
            eprintln!("list_devices failed: {e}");
            std::process::exit(1);
        }
    }
}
