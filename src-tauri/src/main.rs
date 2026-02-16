use csv::StringRecord;
use rfd::FileDialog;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
struct ProcessState {
    child: Arc<Mutex<Option<Child>>>,
    stop_requested: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct ClipRow {
    clip_name: String,
    start_time: String,
    end_time: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClipRowInput {
    clip_name: String,
    start_time: String,
    end_time: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ExportSettings {
    processing_mode: String,
    preset: String,
    crf: u8,
    resolution: String,
    audio_codec: String,
    audio_bitrate_kbps: u16,
    fps: Option<f64>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            processing_mode: "copy_fast".to_string(),
            preset: "ultrafast".to_string(),
            crf: 20,
            resolution: "source".to_string(),
            audio_codec: "aac".to_string(),
            audio_bitrate_kbps: 128,
            fps: None,
        }
    }
}

#[derive(Serialize, Clone)]
struct ProgressPayload {
    total: usize,
    completed: usize,
    current_clip: String,
    status: String,
    message: String,
    row_index: Option<usize>,
    row_result: Option<String>,
}

#[derive(Serialize)]
struct RunSummary {
    total_rows: usize,
    exported: usize,
    skipped: usize,
    failed: usize,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct CsvPreview {
    total_rows: usize,
    rows: Vec<ClipRowPreview>,
    validation_errors: Vec<String>,
}

#[derive(Serialize)]
struct ClipRowPreview {
    clip_name: String,
    start_time: String,
    end_time: String,
}

#[tauri::command]
fn pick_csv_file() -> Option<String> {
    FileDialog::new()
        .add_filter("CSV", &["csv"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_video_file() -> Option<String> {
    FileDialog::new()
        .add_filter("Video", &["mp4", "mov", "mkv", "m4v", "avi"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_output_dir() -> Option<String> {
    FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn preview_csv(csv_path: String) -> Result<CsvPreview, String> {
    let rows = read_clip_rows(&csv_path)?;
    let mut validation_errors = Vec::new();

    for (idx, row) in rows.iter().enumerate() {
        let row_num = idx + 2;
        if row.start_time.trim().is_empty() || row.end_time.trim().is_empty() {
            validation_errors.push(format!("Row {} missing start/end time", row_num));
            continue;
        }

        if convert_to_seconds(&row.start_time).is_none() {
            validation_errors.push(format!(
                "Row {} invalid start time: {}",
                row_num, row.start_time
            ));
        }

        if convert_to_seconds(&row.end_time).is_none() {
            validation_errors.push(format!(
                "Row {} invalid end time: {}",
                row_num, row.end_time
            ));
        }
    }

    let preview_rows = rows
        .iter()
        .map(|r| ClipRowPreview {
            clip_name: r.clip_name.clone(),
            start_time: r.start_time.clone(),
            end_time: r.end_time.clone(),
        })
        .collect::<Vec<_>>();

    Ok(CsvPreview {
        total_rows: rows.len(),
        rows: preview_rows,
        validation_errors,
    })
}

#[tauri::command]
fn stop_export(state: State<ProcessState>) -> Result<(), String> {
    state.stop_requested.store(true, Ordering::SeqCst);
    if let Some(child) = state.child.lock().map_err(|e| e.to_string())?.as_mut() {
        child
            .kill()
            .map_err(|e| format!("Failed to stop ffmpeg: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
async fn start_export(
    app: AppHandle,
    state: State<'_, ProcessState>,
    csv_path: String,
    video_path: String,
    output_dir: String,
    settings: Option<ExportSettings>,
    edited_rows: Option<Vec<ClipRowInput>>,
) -> Result<RunSummary, String> {
    let child_state = state.child.clone();
    let stop_state = state.stop_requested.clone();

    tauri::async_runtime::spawn_blocking(move || {
        run_export(
            app,
            child_state,
            stop_state,
            csv_path,
            video_path,
            output_dir,
            settings.unwrap_or_default(),
            edited_rows,
        )
    })
    .await
    .map_err(|e| format!("Export task failed: {e}"))?
}

fn run_export(
    app: AppHandle,
    child_state: Arc<Mutex<Option<Child>>>,
    stop_state: Arc<AtomicBool>,
    csv_path: String,
    video_path: String,
    output_dir: String,
    raw_settings: ExportSettings,
    edited_rows: Option<Vec<ClipRowInput>>,
) -> Result<RunSummary, String> {
    stop_state.store(false, Ordering::SeqCst);
    let settings = normalize_settings(raw_settings);

    ensure_ffmpeg_exists()?;

    let clip_rows = if let Some(rows) = edited_rows {
        let normalized = rows
            .into_iter()
            .map(|r| ClipRow {
                clip_name: if r.clip_name.trim().is_empty() {
                    "clip".to_string()
                } else {
                    r.clip_name.trim().to_string()
                },
                start_time: r.start_time.trim().to_string(),
                end_time: r.end_time.trim().to_string(),
            })
            .filter(|r| {
                !(r.clip_name.is_empty() && r.start_time.is_empty() && r.end_time.is_empty())
            })
            .collect::<Vec<_>>();

        if normalized.is_empty() {
            return Err("No editable rows to export. Load a CSV first.".to_string());
        }

        normalized
    } else {
        read_clip_rows(&csv_path)?
    };
    let total = clip_rows.len();

    if total == 0 {
        return Err("CSV has no rows".to_string());
    }

    let source_video = PathBuf::from(&video_path);
    if !source_video.exists() {
        return Err(format!("Video file not found: {video_path}"));
    }

    let output_path = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&output_path)
        .map_err(|e| format!("Failed to create output directory: {e}"))?;

    let mut exported = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    emit_progress(
        &app,
        ProgressPayload {
            total,
            completed: 0,
            current_clip: String::new(),
            status: "running".to_string(),
            message: "Starting export...".to_string(),
            row_index: None,
            row_result: None,
        },
    );

    for (idx, row) in clip_rows.iter().enumerate() {
        if stop_state.load(Ordering::SeqCst) {
            emit_progress(
                &app,
                ProgressPayload {
                    total,
                    completed: idx,
                    current_clip: row.clip_name.clone(),
                    status: "stopped".to_string(),
                    message: "Export stopped by user".to_string(),
                    row_index: Some(idx),
                    row_result: Some("failed".to_string()),
                },
            );
            break;
        }

        let start_sec = match convert_to_seconds(&row.start_time) {
            Some(v) => v,
            None => {
                skipped += 1;
                let err = format!(
                    "Row {} skipped: invalid start time '{}'",
                    idx + 2,
                    row.start_time
                );
                errors.push(err.clone());
                emit_progress(
                    &app,
                    ProgressPayload {
                        total,
                        completed: idx + 1,
                        current_clip: row.clip_name.clone(),
                        status: "running".to_string(),
                        message: err,
                        row_index: Some(idx),
                        row_result: Some("failed".to_string()),
                    },
                );
                continue;
            }
        };

        let end_sec = match convert_to_seconds(&row.end_time) {
            Some(v) => v,
            None => {
                skipped += 1;
                let err = format!(
                    "Row {} skipped: invalid end time '{}'",
                    idx + 2,
                    row.end_time
                );
                errors.push(err.clone());
                emit_progress(
                    &app,
                    ProgressPayload {
                        total,
                        completed: idx + 1,
                        current_clip: row.clip_name.clone(),
                        status: "running".to_string(),
                        message: err,
                        row_index: Some(idx),
                        row_result: Some("failed".to_string()),
                    },
                );
                continue;
            }
        };

        if end_sec <= start_sec {
            skipped += 1;
            let err = format!(
                "Row {} skipped: end time must be greater than start time",
                idx + 2
            );
            errors.push(err.clone());
            emit_progress(
                &app,
                ProgressPayload {
                    total,
                    completed: idx + 1,
                    current_clip: row.clip_name.clone(),
                    status: "running".to_string(),
                    message: err,
                    row_index: Some(idx),
                    row_result: Some("failed".to_string()),
                },
            );
            continue;
        }

        let safe_name = sanitize_filename(&row.clip_name);
        let start_label = row.start_time.replace(':', "");
        let output_ext = if settings.processing_mode == "copy_fast" {
            source_video
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "mp4".to_string())
        } else {
            "mp4".to_string()
        };
        let file_name = format!(
            "{:03}-{}-{}.{}",
            idx + 1,
            safe_name,
            start_label,
            output_ext
        );
        let destination = output_path.join(file_name);

        emit_progress(
            &app,
            ProgressPayload {
                total,
                completed: idx,
                current_clip: row.clip_name.clone(),
                status: "running".to_string(),
                message: format!("Exporting clip {} of {}", idx + 1, total),
                row_index: Some(idx),
                row_result: Some("running".to_string()),
            },
        );

        let mut cmd = Command::new("ffmpeg");
        let duration = end_sec - start_sec;
        cmd.arg("-y").arg("-loglevel").arg("error").arg("-nostats");

        match settings.processing_mode.as_str() {
            "copy_fast" => {
                cmd.arg("-ss")
                    .arg(start_sec.to_string())
                    .arg("-i")
                    .arg(&source_video)
                    .arg("-t")
                    .arg(duration.to_string())
                    .arg("-c")
                    .arg("copy");
            }
            "reencode_fast_seek" => {
                cmd.arg("-ss")
                    .arg(start_sec.to_string())
                    .arg("-i")
                    .arg(&source_video)
                    .arg("-t")
                    .arg(duration.to_string())
                    .arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg(&settings.preset)
                    .arg("-crf")
                    .arg(settings.crf.to_string());

                if let Some(filter) = resolution_filter(&settings.resolution) {
                    cmd.arg("-vf").arg(filter);
                }

                if let Some(fps) = settings.fps {
                    cmd.arg("-r").arg(fps.to_string());
                }
            }
            _ => {
                cmd.arg("-i")
                    .arg(&source_video)
                    .arg("-ss")
                    .arg(start_sec.to_string())
                    .arg("-to")
                    .arg(end_sec.to_string())
                    .arg("-c:v")
                    .arg("libx264")
                    .arg("-preset")
                    .arg(&settings.preset)
                    .arg("-crf")
                    .arg(settings.crf.to_string());

                if let Some(filter) = resolution_filter(&settings.resolution) {
                    cmd.arg("-vf").arg(filter);
                }

                if let Some(fps) = settings.fps {
                    cmd.arg("-r").arg(fps.to_string());
                }
            }
        }

        if settings.processing_mode != "copy_fast" {
            match settings.audio_codec.as_str() {
                "none" => {
                    cmd.arg("-an");
                }
                "copy" => {
                    cmd.arg("-c:a").arg("copy");
                }
                _ => {
                    cmd.arg("-c:a")
                        .arg("aac")
                        .arg("-b:a")
                        .arg(format!("{}k", settings.audio_bitrate_kbps));
                }
            }
        }

        if output_ext == "mp4" || output_ext == "m4v" {
            cmd.arg("-movflags").arg("+faststart");
        }

        cmd.arg(&destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start ffmpeg process: {e}"))?;

        {
            let mut guard = child_state.lock().map_err(|e| e.to_string())?;
            *guard = Some(child);
        }

        let output_status = loop {
            let status = {
                let mut guard = child_state.lock().map_err(|e| e.to_string())?;
                let running = guard
                    .as_mut()
                    .ok_or_else(|| "Internal error: ffmpeg process missing".to_string())?;
                running
                    .try_wait()
                    .map_err(|e| format!("Failed waiting on ffmpeg process: {e}"))?
            };

            if let Some(status) = status {
                break status;
            }

            std::thread::sleep(std::time::Duration::from_millis(120));
        };

        {
            let mut guard = child_state.lock().map_err(|e| e.to_string())?;
            let _ = guard.take();
        }

        if stop_state.load(Ordering::SeqCst) {
            failed += 1;
            errors.push(format!("Stopped while exporting row {}", idx + 2));
            break;
        }

        if output_status.success() && destination.exists() {
            exported += 1;
        } else {
            failed += 1;
            errors.push(format!("Row {} failed ({})", idx + 2, row.clip_name));
        }

        emit_progress(
            &app,
            ProgressPayload {
                total,
                completed: idx + 1,
                current_clip: row.clip_name.clone(),
                status: "running".to_string(),
                message: format!("Finished clip {} of {}", idx + 1, total),
                row_index: Some(idx),
                row_result: Some(if output_status.success() && destination.exists() {
                    "success".to_string()
                } else {
                    "failed".to_string()
                }),
            },
        );
    }

    let status = if stop_state.load(Ordering::SeqCst) {
        "stopped"
    } else {
        "done"
    };

    emit_progress(
        &app,
        ProgressPayload {
            total,
            completed: exported + failed + skipped,
            current_clip: String::new(),
            status: status.to_string(),
            message: format!(
                "Done. Exported: {}, Skipped: {}, Failed: {}",
                exported, skipped, failed
            ),
            row_index: None,
            row_result: None,
        },
    );

    Ok(RunSummary {
        total_rows: total,
        exported,
        skipped,
        failed,
        errors,
    })
}

fn emit_progress(app: &AppHandle, payload: ProgressPayload) {
    let _ = app.emit("export-progress", payload);
}

fn ensure_ffmpeg_exists() -> Result<(), String> {
    which::which("ffmpeg")
        .map(|_| ())
        .map_err(|_| "ffmpeg not found in PATH. Install ffmpeg before running exports.".to_string())
}

fn read_clip_rows(csv_path: &str) -> Result<Vec<ClipRow>, String> {
    let path = Path::new(csv_path);
    if !path.exists() {
        return Err(format!("CSV file not found: {csv_path}"));
    }

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .map_err(|e| format!("Failed to open CSV: {e}"))?;

    let headers = reader
        .headers()
        .map_err(|e| format!("Failed reading CSV headers: {e}"))?
        .clone();

    let idx_name = find_header_index(&headers, &["clip name", "name", "clip"])
        .ok_or_else(|| "CSV missing clip name column".to_string())?;
    let idx_start = find_header_index(&headers, &["clip start time", "start time", "start", "in"])
        .ok_or_else(|| "CSV missing clip start time column".to_string())?;
    let idx_end = find_header_index(&headers, &["clip end time", "end time", "end", "out"])
        .ok_or_else(|| "CSV missing clip end time column".to_string())?;

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("Failed reading CSV rows: {e}"))?;
        let clip_name = record.get(idx_name).unwrap_or("clip").trim();
        let start_time = record.get(idx_start).unwrap_or("").trim();
        let end_time = record.get(idx_end).unwrap_or("").trim();

        if clip_name.is_empty() && start_time.is_empty() && end_time.is_empty() {
            continue;
        }

        rows.push(ClipRow {
            clip_name: if clip_name.is_empty() {
                "clip".to_string()
            } else {
                clip_name.to_string()
            },
            start_time: start_time.to_string(),
            end_time: end_time.to_string(),
        });
    }

    Ok(rows)
}

fn find_header_index(headers: &StringRecord, aliases: &[&str]) -> Option<usize> {
    let normalized_aliases = aliases
        .iter()
        .map(|alias| normalize_header(alias))
        .collect::<Vec<_>>();

    let normalized = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (i, normalize_header(h)))
        .collect::<Vec<_>>();

    normalized
        .into_iter()
        .find(|(_, header)| normalized_aliases.iter().any(|alias| header == alias))
        .map(|(i, _)| i)
}

fn normalize_settings(input: ExportSettings) -> ExportSettings {
    let processing_mode = match input.processing_mode.as_str() {
        "reencode_precise" | "copy_fast" | "reencode_fast_seek" => input.processing_mode,
        _ => "copy_fast".to_string(),
    };

    let valid_presets = [
        "ultrafast",
        "superfast",
        "veryfast",
        "faster",
        "fast",
        "medium",
    ];

    let preset = if valid_presets.contains(&input.preset.as_str()) {
        input.preset
    } else {
        "ultrafast".to_string()
    };

    let resolution = match input.resolution.as_str() {
        "source" | "1080p" | "720p" | "480p" => input.resolution,
        _ => "source".to_string(),
    };

    let audio_codec = match input.audio_codec.as_str() {
        "aac" | "copy" | "none" => input.audio_codec,
        _ => "aac".to_string(),
    };

    let audio_bitrate_kbps = input.audio_bitrate_kbps.clamp(64, 320);
    let crf = input.crf.clamp(16, 35);

    let fps = match input.fps {
        Some(value) if value.is_finite() && (1.0..=120.0).contains(&value) => Some(value),
        _ => None,
    };

    ExportSettings {
        processing_mode,
        preset,
        crf,
        resolution,
        audio_codec,
        audio_bitrate_kbps,
        fps,
    }
}

fn resolution_filter(resolution: &str) -> Option<String> {
    let (w, h) = match resolution {
        "1080p" => (1920, 1080),
        "720p" => (1280, 720),
        "480p" => (854, 480),
        _ => return None,
    };

    Some(format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2"
    ))
}

fn normalize_header(input: &str) -> String {
    input
        .trim_start_matches('\u{feff}')
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn convert_to_seconds(ts: &str) -> Option<f64> {
    let ts = ts.trim();
    if ts.is_empty() {
        return None;
    }

    let parts = ts.split(':').collect::<Vec<_>>();

    let result = match parts.len() {
        4 => {
            let h = parts[0].parse::<f64>().ok()?;
            let m = parts[1].parse::<f64>().ok()?;
            let s = parts[2].parse::<f64>().ok()?;
            let f = parts[3].parse::<f64>().ok()?;
            (h * 3600.0) + (m * 60.0) + s + (f / 30.0)
        }
        3 => {
            let h = parts[0].parse::<f64>().ok()?;
            let m = parts[1].parse::<f64>().ok()?;
            let s = parts[2].parse::<f64>().ok()?;
            (h * 3600.0) + (m * 60.0) + s
        }
        2 => {
            let m = parts[0].parse::<f64>().ok()?;
            let s = parts[1].parse::<f64>().ok()?;
            (m * 60.0) + s
        }
        1 => parts[0].parse::<f64>().ok()?,
        _ => return None,
    };

    Some(result)
}

fn sanitize_filename(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            ' ' => '-',
            _ => '-',
        })
        .collect::<String>();

    let compact = cleaned
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if compact.is_empty() {
        "clip".to_string()
    } else {
        compact
    }
}

fn main() {
    tauri::Builder::default()
        .manage(ProcessState::default())
        .invoke_handler(tauri::generate_handler![
            pick_csv_file,
            pick_video_file,
            pick_output_dir,
            preview_csv,
            start_export,
            stop_export
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
