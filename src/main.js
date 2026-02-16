const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const csvPathInput = document.getElementById("csvPath");
const videoPathInput = document.getElementById("videoPath");
const outputDirInput = document.getElementById("outputDir");
const previewMeta = document.getElementById("previewMeta");
const previewBody = document.querySelector("#previewTable tbody");
const progressBar = document.getElementById("progressBar");
const progressText = document.getElementById("progressText");
const log = document.getElementById("log");
const csvHelpBtn = document.getElementById("csvHelpBtn");
const csvHelpPanel = document.getElementById("csvHelpPanel");
const addRowBtn = document.getElementById("addRowBtn");
const removeRowBtn = document.getElementById("removeRowBtn");

const pickCsvBtn = document.getElementById("pickCsvBtn");
const pickVideoBtn = document.getElementById("pickVideoBtn");
const pickOutputBtn = document.getElementById("pickOutputBtn");
const startBtn = document.getElementById("startBtn");
const stopBtn = document.getElementById("stopBtn");
const processingModeInput = document.getElementById("processingMode");
const modeHint = document.getElementById("modeHint");
const resolutionInput = document.getElementById("resolution");
const presetInput = document.getElementById("preset");
const crfInput = document.getElementById("crf");
const audioCodecInput = document.getElementById("audioCodec");
const audioBitrateInput = document.getElementById("audioBitrate");
const fpsInput = document.getElementById("fps");

let running = false;
let editableRows = [];
let selectedRowIndex = -1;
const videoExtensions = [".mp4", ".mov", ".mkv", ".m4v", ".avi"];
const dropInputs = [csvPathInput, videoPathInput, outputDirInput];

function updateAudioBitrateState() {
  audioBitrateInput.disabled = audioCodecInput.value !== "aac" || running;
}

function updateModeControlState() {
  const mode = processingModeInput.value;
  resolutionInput.disabled = running;
  presetInput.disabled = running;
  crfInput.disabled = running;
  fpsInput.disabled = running;
  audioCodecInput.disabled = running;

  if (mode === "copy_fast") {
    modeHint.textContent = "Copy Streams mode is fastest and keeps source resolution and container/extension. Re-encode controls (resolution, preset, CRF, FPS, audio re-encode options) are ignored in this mode.";
  } else if (mode === "reencode_fast_seek") {
    modeHint.textContent = "Fast Seek mode re-encodes and applies your quality/resolution/audio settings, usually faster than precise mode.";
  } else {
    modeHint.textContent = "Precise mode re-encodes and prioritizes cut accuracy. All encoding settings apply.";
  }

  updateAudioBitrateState();
}

function appendLog(message) {
  const now = new Date();
  const ts = now.toLocaleTimeString();
  log.textContent = `[${ts}] ${message}\n${log.textContent}`.trim();
}

function setRunning(value) {
  running = value;
  startBtn.disabled = value;
  stopBtn.disabled = !value;
  pickCsvBtn.disabled = value;
  pickVideoBtn.disabled = value;
  pickOutputBtn.disabled = value;
  csvHelpBtn.disabled = value;
  processingModeInput.disabled = value;
  for (const input of previewBody.querySelectorAll(".cell-input")) {
    input.disabled = value;
  }
  addRowBtn.disabled = value;
  removeRowBtn.disabled = value || editableRows.length === 0;
  updateModeControlState();
}

function statusMeta(status) {
  switch (status) {
    case "running":
      return { icon: "●", className: "status-running", label: "Running" };
    case "success":
      return { icon: "✓", className: "status-success", label: "Complete" };
    case "failed":
      return { icon: "✕", className: "status-failed", label: "Failed" };
    default:
      return { icon: "○", className: "status-pending", label: "Pending" };
  }
}

function renderPreview(rows) {
  previewBody.innerHTML = "";
  for (let i = 0; i < rows.length; i += 1) {
    const row = rows[i];
    const status = statusMeta(row._status || "pending");
    const selectedClass = i === selectedRowIndex ? "row-selected" : "";
    const tr = document.createElement("tr");
    tr.className = selectedClass;
    tr.dataset.row = String(i);
    tr.innerHTML = `
      <td><span class="status-dot ${status.className}" title="${status.label}">${status.icon}</span></td>
      <td class="row-number">${i + 1}</td>
      <td><input class="cell-input" data-row="${i}" data-field="clip_name" value="${escapeHtml(row.clip_name)}" /></td>
      <td><input class="cell-input" data-row="${i}" data-field="start_time" value="${escapeHtml(row.start_time)}" /></td>
      <td><input class="cell-input" data-row="${i}" data-field="end_time" value="${escapeHtml(row.end_time)}" /></td>
      <td><button class="row-remove-btn" type="button" data-action="remove-row" data-row="${i}" ${running ? "disabled" : ""}>-</button></td>
    `;
    previewBody.appendChild(tr);
  }
  removeRowBtn.disabled = running || rows.length === 0;
}

function escapeHtml(input) {
  return input
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function isCsvPath(path) {
  return path.toLowerCase().endsWith(".csv");
}

function isVideoPath(path) {
  const lower = path.toLowerCase();
  return videoExtensions.some((ext) => lower.endsWith(ext));
}

function hasKnownFileExt(path) {
  return /\.[a-z0-9]{2,6}$/i.test(path);
}

function clearDropActive() {
  for (const input of dropInputs) {
    input.classList.remove("drop-input-active");
    input.closest(".drop-field")?.classList.remove("drop-field-active");
  }
}

function dropKindFromElement(el) {
  if (!el) {
    return null;
  }
  const directInput = el.closest?.("input[data-drop-kind]");
  if (directInput?.dataset?.dropKind) {
    return directInput.dataset.dropKind;
  }

  const dropField = el.closest?.(".drop-field");
  const fieldInput = dropField?.querySelector?.("input[data-drop-kind]");
  return fieldInput?.dataset?.dropKind || null;
}

function dropKindFromRects(x, y) {
  const tolerance = 10;
  for (const input of dropInputs) {
    const field = input.closest(".drop-field") || input;
    const rect = field.getBoundingClientRect();
    const inside =
      x >= rect.left - tolerance &&
      x <= rect.right + tolerance &&
      y >= rect.top - tolerance &&
      y <= rect.bottom + tolerance;
    if (inside) {
      return input.dataset.dropKind || null;
    }
  }
  return null;
}

function getDropKindAtPosition(position) {
  if (!position) {
    return null;
  }

  const scale = window.devicePixelRatio || 1;
  const candidates = [
    { x: position.x, y: position.y },
    { x: position.x / scale, y: position.y / scale }
  ];

  for (const point of candidates) {
    const el = document.elementFromPoint(point.x, point.y);
    const kindFromElement = dropKindFromElement(el);
    if (kindFromElement) {
      return kindFromElement;
    }

    const kindFromRect = dropKindFromRects(point.x, point.y);
    if (kindFromRect) {
      return kindFromRect;
    }
  }

  return null;
}

function firstValidPathForKind(paths, kind) {
  if (kind === "csv") {
    return paths.find((p) => isCsvPath(p)) || null;
  }
  if (kind === "video") {
    return paths.find((p) => isVideoPath(p)) || null;
  }
  if (kind === "output") {
    return paths.find((p) => !hasKnownFileExt(p)) || null;
  }
  return null;
}

async function applyDroppedPathToKind(path, kind) {
  if (kind === "csv") {
    csvPathInput.value = path;
    appendLog(`Dropped CSV: ${path}`);
    await loadCsvPreview(path);
    return;
  }

  if (kind === "video") {
    videoPathInput.value = path;
    appendLog(`Dropped video: ${path}`);
    return;
  }

  if (kind === "output") {
    outputDirInput.value = path;
    appendLog(`Dropped output directory: ${path}`);
  }
}

function readSettings() {
  const rawCrf = Number.parseInt(crfInput.value, 10);
  const rawAudioBitrate = Number.parseInt(audioBitrateInput.value, 10);
  const rawFps = fpsInput.value.trim();

  const crf = Number.isFinite(rawCrf) ? Math.max(16, Math.min(35, rawCrf)) : 20;
  const audio_bitrate_kbps = Number.isFinite(rawAudioBitrate) ? Math.max(64, Math.min(320, rawAudioBitrate)) : 128;
  const fps = rawFps === "" ? null : Number.parseFloat(rawFps);

  return {
    processing_mode: processingModeInput.value,
    resolution: resolutionInput.value,
    preset: presetInput.value,
    crf,
    audio_codec: audioCodecInput.value,
    audio_bitrate_kbps,
    fps: Number.isFinite(fps) ? fps : null
  };
}

function getEditedRowsForExport() {
  return editableRows.map((row) => ({
    clip_name: (row.clip_name || "").trim(),
    start_time: (row.start_time || "").trim(),
    end_time: (row.end_time || "").trim()
  }));
}

function resetRowStatuses() {
  for (const row of editableRows) {
    row._status = "pending";
  }
  renderPreview(editableRows);
}

function setRowStatus(index, status) {
  if (!Number.isInteger(index) || index < 0 || index >= editableRows.length) {
    return;
  }
  editableRows[index]._status = status;
  renderPreview(editableRows);
}

async function loadCsvPreview(csvPath) {
  try {
    const preview = await invoke("preview_csv", { csvPath });
    editableRows = (Array.isArray(preview.rows) ? preview.rows : []).map((row) => ({
      ...row,
      _status: "pending"
    }));
    selectedRowIndex = editableRows.length > 0 ? 0 : -1;
    renderPreview(editableRows);

    const errorCount = preview.validation_errors.length;
    previewMeta.textContent = `${preview.total_rows} rows loaded. Editable table ready. Validation issues: ${errorCount}.`;

    if (errorCount > 0) {
      appendLog(`CSV validation: ${errorCount} issue(s). First: ${preview.validation_errors[0]}`);
    } else {
      appendLog(`CSV validation passed for ${preview.total_rows} rows.`);
    }
  } catch (error) {
    previewMeta.textContent = "Failed to preview CSV";
    previewBody.innerHTML = "";
    appendLog(`Error previewing CSV: ${error}`);
  }
}

pickCsvBtn.addEventListener("click", async () => {
  const path = await invoke("pick_csv_file");
  if (!path) {
    return;
  }

  csvPathInput.value = path;
  appendLog(`Selected CSV: ${path}`);
  await loadCsvPreview(path);
});

pickVideoBtn.addEventListener("click", async () => {
  const path = await invoke("pick_video_file");
  if (!path) {
    return;
  }

  videoPathInput.value = path;
  appendLog(`Selected video: ${path}`);
});

pickOutputBtn.addEventListener("click", async () => {
  const path = await invoke("pick_output_dir");
  if (!path) {
    return;
  }

  outputDirInput.value = path;
  appendLog(`Selected output dir: ${path}`);
});

startBtn.addEventListener("click", async () => {
  const csvPath = csvPathInput.value;
  const videoPath = videoPathInput.value;
  const outputDir = outputDirInput.value;

  if (!csvPath || !videoPath || !outputDir) {
    appendLog("Select CSV file, source video, and output directory before starting.");
    return;
  }
  if (editableRows.length === 0) {
    appendLog("No rows loaded. Select a CSV first.");
    return;
  }
  resetRowStatuses();

  setRunning(true);
  progressBar.value = 0;
  progressText.textContent = "Starting export...";
  const settings = readSettings();
  const editedRows = getEditedRowsForExport();

  try {
    appendLog(
      `Encoding settings: mode=${settings.processing_mode}, ${settings.resolution}, ${settings.preset}, CRF ${settings.crf}, audio ${settings.audio_codec}${settings.audio_codec === "aac" ? ` ${settings.audio_bitrate_kbps}k` : ""}${settings.fps ? `, ${settings.fps}fps` : ""}.`
    );
    const summary = await invoke("start_export", { csvPath, videoPath, outputDir, settings, editedRows });
    appendLog(`Completed. Exported ${summary.exported}, skipped ${summary.skipped}, failed ${summary.failed}.`);
    if (summary.errors.length > 0) {
      appendLog(`First error: ${summary.errors[0]}`);
    }
  } catch (error) {
    appendLog(`Export error: ${error}`);
  } finally {
    setRunning(false);
  }
});

stopBtn.addEventListener("click", async () => {
  try {
    await invoke("stop_export");
    appendLog("Stop requested.");
  } catch (error) {
    appendLog(`Failed to stop: ${error}`);
  }
});

previewBody.addEventListener("input", (event) => {
  const target = event.target;
  if (!(target instanceof HTMLInputElement)) {
    return;
  }

  const rowIndex = Number.parseInt(target.dataset.row || "", 10);
  const field = target.dataset.field;
  if (!Number.isInteger(rowIndex) || rowIndex < 0 || rowIndex >= editableRows.length) {
    return;
  }
  if (!["clip_name", "start_time", "end_time"].includes(field)) {
    return;
  }

  editableRows[rowIndex][field] = target.value;
});

previewBody.addEventListener("click", (event) => {
  const removeBtn = event.target?.closest?.("button[data-action='remove-row']");
  if (removeBtn) {
    const idx = Number.parseInt(removeBtn.dataset.row || "", 10);
    if (Number.isInteger(idx) && idx >= 0 && idx < editableRows.length) {
      editableRows.splice(idx, 1);
      if (editableRows.length === 0) {
        selectedRowIndex = -1;
      } else {
        selectedRowIndex = Math.min(idx, editableRows.length - 1);
      }
      renderPreview(editableRows);
    }
    return;
  }

  const tr = event.target?.closest?.("tr[data-row]");
  if (!tr) {
    return;
  }
  const idx = Number.parseInt(tr.dataset.row || "", 10);
  if (!Number.isInteger(idx)) {
    return;
  }
  selectedRowIndex = idx;
  renderPreview(editableRows);
});

previewBody.addEventListener("focusin", (event) => {
  const target = event.target;
  if (!(target instanceof HTMLElement)) {
    return;
  }
  const tr = target.closest("tr[data-row]");
  if (!tr) {
    return;
  }
  const idx = Number.parseInt(tr.dataset.row || "", 10);
  if (!Number.isInteger(idx)) {
    return;
  }
  if (idx !== selectedRowIndex) {
    selectedRowIndex = idx;
    renderPreview(editableRows);
  }
});

csvHelpBtn.addEventListener("click", () => {
  csvHelpPanel.hidden = !csvHelpPanel.hidden;
});

addRowBtn.addEventListener("click", () => {
  editableRows.push({
    clip_name: "New Clip",
    start_time: "00:00:00",
    end_time: "00:00:05",
    _status: "pending"
  });
  selectedRowIndex = editableRows.length - 1;
  renderPreview(editableRows);
  removeRowBtn.disabled = running || editableRows.length === 0;
});

removeRowBtn.addEventListener("click", () => {
  if (editableRows.length === 0) {
    return;
  }

  const idx = selectedRowIndex >= 0 ? selectedRowIndex : editableRows.length - 1;
  editableRows.splice(idx, 1);
  if (editableRows.length === 0) {
    selectedRowIndex = -1;
  } else {
    selectedRowIndex = Math.min(idx, editableRows.length - 1);
  }
  renderPreview(editableRows);
  removeRowBtn.disabled = running || editableRows.length === 0;
});

audioCodecInput.addEventListener("change", () => {
  updateAudioBitrateState();
});
processingModeInput.addEventListener("change", () => {
  updateModeControlState();
});

async function initProgressListener() {
  try {
    await listen("export-progress", (event) => {
      const payload = event.payload;
      if (!payload || !payload.total) {
        return;
      }

      const percentage = Math.round((payload.completed / payload.total) * 100);
      progressBar.value = Math.min(100, Math.max(0, percentage));
      progressText.textContent = `${payload.message} (${payload.completed}/${payload.total})`;

      if (payload.status === "done") {
        progressBar.value = 100;
      }

      if (payload.status === "stopped") {
        appendLog("Export stopped.");
        setRunning(false);
      }

      if (Number.isInteger(payload.row_index) && payload.row_result) {
        const mapped =
          payload.row_result === "success"
            ? "success"
            : payload.row_result === "failed" || payload.row_result === "skipped"
              ? "failed"
              : payload.row_result === "running"
                ? "running"
                : "pending";
        setRowStatus(payload.row_index, mapped);
        if (mapped === "failed") {
          appendLog(`Row ${payload.row_index + 1} failed: ${payload.message}`);
        }
      }
    });
    appendLog("Progress listener ready.");
  } catch (error) {
    appendLog(`Progress listener failed: ${error}`);
  }
}

initProgressListener();
updateModeControlState();
removeRowBtn.disabled = true;

async function initDragDrop() {
  try {
    await listen("tauri://drag-over", (event) => {
      clearDropActive();
      const kind = getDropKindAtPosition(event.payload?.position);
      const targetInput = document.querySelector(`input[data-drop-kind="${kind}"]`);
      if (targetInput) {
        targetInput.classList.add("drop-input-active");
        targetInput.closest(".drop-field")?.classList.add("drop-field-active");
      }
    });
    await listen("tauri://drag-leave", () => clearDropActive());
    await listen("tauri://drag-drop", async (event) => {
      clearDropActive();
      const paths = event.payload?.paths || [];
      const kind = getDropKindAtPosition(event.payload?.position);
      if (!kind) {
        appendLog("Drop ignored: place the file/folder directly onto the intended input box.");
        return;
      }

      const selectedPath = firstValidPathForKind(paths, kind);
      if (!selectedPath) {
        if (kind === "csv") {
          appendLog("Drop ignored: CSV input only accepts .csv files.");
        } else if (kind === "video") {
          appendLog("Drop ignored: Video input expects .mp4/.mov/.mkv/.m4v/.avi.");
        } else {
          appendLog("Drop ignored: Output input expects a directory.");
        }
        return;
      }

      await applyDroppedPathToKind(selectedPath, kind);
    });
    appendLog("Targeted drag and drop ready.");
  } catch (error) {
    appendLog(`Drag and drop unavailable: ${error}`);
  }
}

initDragDrop();

function initSettingHelpPopover() {
  const helpButtons = Array.from(document.querySelectorAll(".setting-help-btn"));
  if (helpButtons.length === 0) {
    return;
  }

  const popover = document.createElement("div");
  popover.className = "help-popover";
  popover.hidden = true;
  document.body.appendChild(popover);

  let activeButton = null;

  function closePopover() {
    popover.hidden = true;
    activeButton = null;
  }

  function openPopover(button) {
    const message = button.getAttribute("title") || "No help text available.";
    popover.textContent = message;
    popover.hidden = false;
    activeButton = button;

    const rect = button.getBoundingClientRect();
    const margin = 8;
    const maxLeft = window.innerWidth - popover.offsetWidth - margin;
    const left = Math.max(margin, Math.min(rect.left, maxLeft));
    const top = Math.min(window.innerHeight - popover.offsetHeight - margin, rect.bottom + 6);
    popover.style.left = `${left}px`;
    popover.style.top = `${Math.max(margin, top)}px`;
  }

  for (const button of helpButtons) {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (activeButton === button && !popover.hidden) {
        closePopover();
      } else {
        openPopover(button);
      }
    });
  }

  document.addEventListener("click", (event) => {
    if (popover.hidden) {
      return;
    }
    const target = event.target;
    if (target instanceof Node && !popover.contains(target)) {
      closePopover();
    }
  });

  window.addEventListener("resize", closePopover);
  window.addEventListener("scroll", closePopover, true);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closePopover();
    }
  });
}

initSettingHelpPopover();
