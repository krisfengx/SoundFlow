# -*- coding: utf-8 -*-
r"""滚动歌词播放器（迁移自 Huhu AI player 页面形态）。

生成自包含 HTML：左侧封面/歌名/歌手/进度条，右侧逐行滚动歌词 +
字级渐进高亮（与 ASS \kf 同款语义）。音频用相对路径引用
（与播放器 HTML 同目录的音频文件），双击即用浏览器打开。
"""
from __future__ import annotations

import json
from pathlib import Path

from lyrics.schema import Lyrics

_TEMPLATE = r"""<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<style>
  :root { --hl: #e8b4bc; --fg: #f2f2f7; --dim: rgba(242,242,247,.35); --mid: rgba(242,242,247,.6); }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { background: #1c1c22; color: var(--fg);
         font-family: "PingFang SC", "Microsoft YaHei", sans-serif;
         height: 100vh; display: flex; overflow: hidden; }
  .left { width: 40%; min-width: 280px; display: flex; flex-direction: column;
          justify-content: center; align-items: center; padding: 40px; gap: 18px; }
  .cover { width: min(320px, 60%); aspect-ratio: 1; background: #6a5c68;
           border-radius: 24px; display: flex; align-items: center; justify-content: center; }
  .cover svg { width: 55%; height: 55%; opacity: .55; }
  .meta { text-align: left; width: min(320px, 60%); }
  .meta .name { font-size: 24px; font-weight: 700; }
  .meta .artist { color: var(--mid); font-size: 16px; margin-top: 4px; }
  .controls { width: min(320px, 60%); display: flex; align-items: center; gap: 10px; }
  .time { color: var(--mid); font-size: 12px; min-width: 34px; text-align: center;
          font-variant-numeric: tabular-nums; }
  /* 平台式可点击/拖动进度条（网易云/Spotify 风） */
  .bar { flex: 1; position: relative; height: 18px; cursor: pointer; touch-action: none; }
  .bar-track { position: absolute; left: 0; right: 0; top: 50%; height: 4px;
               transform: translateY(-50%); background: rgba(242,242,247,.22);
               border-radius: 3px; transition: height .15s; }
  .bar:hover .bar-track, .bar.dragging .bar-track { height: 6px; }
  .bar-fill { position: absolute; left: 0; top: 0; bottom: 0; width: 0%;
              background: var(--hl); border-radius: 3px; }
  .bar-thumb { position: absolute; top: 50%; left: 0; width: 12px; height: 12px;
               border-radius: 50%; background: #fff;
               box-shadow: 0 1px 4px rgba(0,0,0,.45);
               transform: translate(-50%, -50%); opacity: 0; transition: opacity .15s;
               pointer-events: none; }
  .bar:hover .bar-thumb, .bar.dragging .bar-thumb { opacity: 1; }
  #play { width: 44px; height: 44px; border-radius: 50%; border: none; cursor: pointer;
          background: var(--hl); color: #fff; font-size: 18px; }
  .right { flex: 1; overflow: hidden; position: relative;
           background: linear-gradient(135deg, #242028 0%, #1c1c22 100%); }
  .scroll { position: absolute; inset: 0; overflow-y: auto; padding: 20vh 48px 60vh; }
  .line { font-size: clamp(20px, 2.6vw, 34px); font-weight: 700; margin: 28px 0;
          color: var(--dim); transition: color .3s, transform .3s; transform-origin: left center;
          cursor: pointer; }
  .line.past { color: var(--mid); }
  .line.active { color: var(--fg); transform: scale(1.06); }
  /* 字级 \kf 渐进填充：当前字从左到右流动填色（与 ASS 同语义） */
  .line .char { color: var(--dim); }
  .line.past .char { color: var(--mid); }
  .line.active .char {
    background: linear-gradient(to right, var(--hl) 0%, var(--hl) var(--p, 0%), var(--dim) var(--p, 0%));
    -webkit-background-clip: text; background-clip: text;
    color: transparent; -webkit-text-fill-color: transparent;
  }
  /* 仅逐行模式：当前行整行纯色高亮 + 柔和背景条 */
  body.mode-line .line.active {
    background: rgba(232, 180, 188, 0.10);
    border-radius: 12px; padding: 8px 14px; margin-left: -14px;
  }
  body.mode-line .line.active .char {
    background: none; -webkit-background-clip: initial; background-clip: initial;
    color: var(--fg); -webkit-text-fill-color: currentColor;
  }
  .bg-line { font-size: clamp(14px, 1.6vw, 20px); font-weight: 500; }
</style>
</head>
<body class="__MODE_CLS__">
  <div class="left">
    <div class="cover">
      <svg viewBox="0 0 24 24" fill="#8a7888"><path d="M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z"/></svg>
    </div>
    <div class="meta">
      <div class="name">__TITLE__</div>
      <div class="artist">__ARTIST__</div>
    </div>
    <div class="controls">
      <span class="time" id="tCur">0:00</span>
      <div class="bar" id="bar">
        <div class="bar-track"><div class="bar-fill" id="barFill"></div></div>
        <div class="bar-thumb" id="barThumb"></div>
      </div>
      <span class="time" id="tEnd">0:00</span>
      <button id="play">▶</button>
    </div>
    __MEDIA_TAG__
  </div>
  <div class="right"><div class="scroll" id="scroll"></div></div>
<script>
const MODE = '__MODE__';  // karaoke=逐字填充 | line=仅逐行
const DATA = __DATA__;
const audio = document.getElementById('audio');
const scroll = document.getElementById('scroll');
const play = document.getElementById('play');
const bar = document.getElementById('bar');
const barFill = document.getElementById('barFill');
const barThumb = document.getElementById('barThumb');
const tCur = document.getElementById('tCur');
let els = [], activeIdx = -1, dragging = false;

DATA.lines.forEach((ln, i) => {
  const div = document.createElement('div');
  div.className = 'line' + (ln.bg ? ' bg-line' : '');
  div.dataset.i = i;
  ln.words.forEach(w => {
    const c = document.createElement('span');
    c.className = 'char'; c.textContent = w.char;
    div.appendChild(c);
  });
  div.addEventListener('click', () => { audio.currentTime = ln.start / 1000; audio.play(); });
  scroll.appendChild(div);
  els.push(div);
});

function fmt(s) { s = Math.max(0, s|0); return (s/60|0) + ':' + String(s%60).padStart(2,'0'); }

function _fill(el, p) {
  el.style.setProperty('--p', Math.min(100, Math.max(0, p)).toFixed(1) + '%');
}

function tick() {
  const t = audio.currentTime * 1000;
  if (!dragging) {
    tCur.textContent = fmt(audio.currentTime);
    if (audio.duration) _setBar(audio.currentTime / audio.duration);
  }
  let idx = -1;
  DATA.lines.forEach((ln, i) => { if (t >= ln.start && t < ln.end + 300) idx = i; });
  els.forEach((el, i) => {
    el.classList.toggle('active', i === idx);
    el.classList.toggle('past', i < idx && i !== idx);
    if (i === idx && activeIdx !== idx)
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    if (i === idx) {
      const chars = el.querySelectorAll('.char');
      if (MODE === 'line') {
        chars.forEach((c) => _fill(c, 100));  // 仅逐行：整行高亮
      } else {
        DATA.lines[i].words.forEach((w, k) => {
          const c = chars[k]; if (!c) return;
          if (t >= w.end) _fill(c, 100);
          else if (t >= w.start)
            _fill(c, (t - w.start) / Math.max(w.end - w.start, 1) * 100);
          else _fill(c, 0);
        });
      }
    }
  });
  activeIdx = idx;
  requestAnimationFrame(tick);
}
requestAnimationFrame(tick);


play.addEventListener('click', () => audio.paused ? audio.play() : audio.pause());
audio.addEventListener('play', () => play.textContent = '⏸');
audio.addEventListener('pause', () => play.textContent = '▶');
audio.addEventListener('loadedmetadata', () => document.getElementById('tEnd').textContent = fmt(audio.duration));

// ---- 平台式进度条：点击任意位置定位 + 按住拖动刮擦（松手才真正 seek） ----
function _setBar(ratio) {
  const p = Math.min(100, Math.max(0, ratio * 100));
  barFill.style.width = p + '%';
  barThumb.style.left = p + '%';
}
function _ratioFromEvent(e) {
  const rect = bar.getBoundingClientRect();
  return Math.min(1, Math.max(0, (e.clientX - rect.left) / Math.max(rect.width, 1)));
}
bar.addEventListener('pointerdown', (e) => {
  if (!audio.duration || !isFinite(audio.duration)) return;
  dragging = true;
  bar.classList.add('dragging');
  bar.setPointerCapture(e.pointerId);
  const r = _ratioFromEvent(e);
  _setBar(r);
  tCur.textContent = fmt(r * audio.duration);
});
bar.addEventListener('pointermove', (e) => {
  if (!dragging) return;
  const r = _ratioFromEvent(e);
  _setBar(r);
  tCur.textContent = fmt(r * audio.duration);
});
bar.addEventListener('pointerup', (e) => {
  if (!dragging) return;
  dragging = false;
  bar.classList.remove('dragging');
  if (audio.duration && isFinite(audio.duration))
    audio.currentTime = _ratioFromEvent(e) * audio.duration;
});
bar.addEventListener('pointercancel', () => {
  dragging = false;
  bar.classList.remove('dragging');
});

</script>
</body>
</html>
"""


def build_player_html(lyrics: Lyrics | None = None, *, lines_data: list | None = None,
                      audio_file: str | None = None,
                      title: str, artist: str, mode: str = "karaoke") -> str:
    """Lyrics → 自包含滚动歌词播放器 HTML。

    audio_file：音频文件名（相对引用，与 HTML 同目录）。
    """
    if lines_data is not None:
        data = {"lines": lines_data}
    else:
        data = {
            "lines": [
                {
                    "start": ln.start,
                    "end": max(ln.end, ln.start + 200),
                    "bg": bool(ln.is_background),
                    "words": [{"char": w.char, "start": w.start, "end": w.end}
                              for w in ln.words],
                }
                for ln in lyrics.lines if ln.words
            ],
        }
    html = _TEMPLATE
    html = html.replace("__TITLE__", title or "Untitled")
    html = html.replace("__ARTIST__", artist or "")
    # 媒体源：纯音频(≤30MB)→base64 内嵌（双击即播）；视频→同目录相对引用
    # （隐藏 <video> 播音轨）；都不可用→空源（仅歌词预览）
    media_tag = '<audio id="audio" src=""></audio>'
    if audio_file:
        src = Path(audio_file)
        ext = src.suffix.lstrip(".").lower()
        audio_exts = {"mp3", "wav", "m4a", "flac", "ogg"}
        video_exts = {"mp4", "mov", "webm", "mkv"}
        if ext in audio_exts and src.is_file() and src.stat().st_size <= 30 * 1024 * 1024:
            import base64
            mime = {"mp3": "audio/mpeg", "wav": "audio/wav", "m4a": "audio/mp4",
                    "flac": "audio/flac", "ogg": "audio/ogg"}.get(ext, "audio/mpeg")
            b64 = base64.b64encode(src.read_bytes()).decode("ascii")
            media_tag = f'<audio id="audio" src="data:{mime};base64,{b64}"></audio>'
        elif ext in video_exts and src.is_file():
            media_tag = (f'<video id="audio" src="{src.name}" style="display:none" '
                         f'preload="auto"></video>')
        elif src.is_file():
            media_tag = f'<audio id="audio" src="{src.name}"></audio>'
    html = html.replace("__MEDIA_TAG__", media_tag)
    html = html.replace("__DATA__", json.dumps(data, ensure_ascii=False))
    html = html.replace("__MODE__", "line" if mode == "line" else "karaoke")
    html = html.replace("__MODE_CLS__", "mode-line" if mode == "line" else "mode-karaoke")
    return html


def save_player(lyrics: Lyrics, audio_file: str | None, out_path: Path,
                *, title: str = "", artist: str = "", mode: str = "karaoke") -> Path:
    out_path = Path(out_path)
    out_path.write_text(build_player_html(lyrics, audio_file=audio_file,
                                          title=title, artist=artist, mode=mode),
                        encoding="utf-8")
    return out_path


def save_lines_player(rows: list[dict], audio_file: str | None, out_path: Path,
                      *, title: str = "", artist: str = "") -> Path:
    """LRC 行级时间（[{text,start_ms,end_ms}]）→ 仅逐行播放器。

    每行作为一个整体高亮单元（无字级渐变），行间平滑滚动。
    """
    lines_data = [
        {
            "start": int(r["start_ms"]),
            "end": max(int(r.get("end_ms") or 0), int(r["start_ms"]) + 500),
            "bg": False,
            "words": [{"char": str(r["text"]),
                       "start": int(r["start_ms"]),
                       "end": max(int(r.get("end_ms") or 0), int(r["start_ms"]) + 500)}],
        }
        for r in rows if str(r.get("text") or "").strip()
    ]
    out_path = Path(out_path)
    out_path.write_text(
        build_player_html(None, lines_data=lines_data, audio_file=audio_file,
                          title=title, artist=artist, mode="line"),
        encoding="utf-8")
    return out_path


# ---------------------------------------------------------------------------
# 双轨 KTV 播放器：人声轨 + 伴奏轨，Web Audio API 单时钟混音，可切「原唱/伴唱」。
#
# 方案复刻自 karaoke-main/static/js/ktv.js：两轨 decodeAudioData 成 AudioBuffer，
# 在同一个 AudioContext 内用同一 startTime/offset 启动两个 BufferSource（采样级
# 对齐）；切换只改人声 GainNode.gain（播放头不动，绝无错位）。本地 file:// 无法
# fetch 相对路径音频，故两轨以 base64 内嵌为 data URI，用 fetch(dataURI) 取字节。
# ---------------------------------------------------------------------------

_DUET_TEMPLATE = r"""<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<style>
  :root { --hl: #e8b4bc; --fg: #f2f2f7; --dim: rgba(242,242,247,.35); --mid: rgba(242,242,247,.6); }
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { background: #1c1c22; color: var(--fg);
         font-family: "PingFang SC", "Microsoft YaHei", sans-serif;
         height: 100vh; display: flex; overflow: hidden; }
  .left { width: 40%; min-width: 280px; display: flex; flex-direction: column;
          justify-content: center; align-items: center; padding: 40px; gap: 14px; }
  .cover { width: min(280px, 55%); aspect-ratio: 1; background: #6a5c68;
           border-radius: 24px; display: flex; align-items: center; justify-content: center; }
  .cover svg { width: 55%; height: 55%; opacity: .55; }
  .meta { text-align: left; width: min(320px, 60%); }
  .meta .name { font-size: 24px; font-weight: 700; }
  .meta .artist { color: var(--mid); font-size: 16px; margin-top: 4px; }
  .controls { width: min(320px, 60%); display: flex; align-items: center; gap: 10px; }
  .time { color: var(--mid); font-size: 12px; min-width: 34px; text-align: center;
          font-variant-numeric: tabular-nums; }
  .bar { flex: 1; position: relative; height: 18px; cursor: pointer; touch-action: none; }
  .bar-track { position: absolute; left: 0; right: 0; top: 50%; height: 4px;
               transform: translateY(-50%); background: rgba(242,242,247,.22);
               border-radius: 3px; transition: height .15s; }
  .bar:hover .bar-track, .bar.dragging .bar-track { height: 6px; }
  .bar-fill { position: absolute; left: 0; top: 0; bottom: 0; width: 0%;
              background: var(--hl); border-radius: 3px; }
  .bar-thumb { position: absolute; top: 50%; left: 0; width: 12px; height: 12px;
               border-radius: 50%; background: #fff; transform: translate(-50%, -50%);
               opacity: 0; transition: opacity .15s; pointer-events: none; }
  .bar:hover .bar-thumb, .bar.dragging .bar-thumb { opacity: 1; }
  #play { width: 44px; height: 44px; border-radius: 50%; border: none; cursor: pointer;
          background: var(--hl); color: #fff; font-size: 18px; flex-shrink: 0; }
  .track-switch { display: flex; gap: 6px; width: min(320px, 60%); }
  .track-switch button { flex: 1; padding: 8px 0; border-radius: 10px; cursor: pointer;
    border: 1px solid rgba(242,242,247,.18); background: rgba(242,242,247,.06);
    color: var(--mid); font-size: 13px; transition: background .15s, color .15s; }
  .track-switch button.on { background: var(--hl); color: #fff; border-color: transparent; }
  .vol-row { width: min(320px, 60%); display: flex; align-items: center; gap: 8px; }
  .vol-row label { font-size: 11px; color: var(--mid); width: 42px; flex-shrink: 0; }
  .vol-row input[type=range] { flex: 1; accent-color: var(--hl); }
  .status { width: min(320px, 60%); font-size: 11px; color: var(--mid); min-height: 14px; }
  .right { flex: 1; overflow: hidden; position: relative;
           background: linear-gradient(135deg, #242028 0%, #1c1c22 100%); }
  .scroll { position: absolute; inset: 0; overflow-y: auto; padding: 20vh 48px 60vh; }
  .line { font-size: clamp(20px, 2.6vw, 34px); font-weight: 700; margin: 28px 0;
          color: var(--dim); transition: color .3s, transform .3s; transform-origin: left center;
          cursor: pointer; }
  .line.past { color: var(--mid); }
  .line.active { color: var(--fg); transform: scale(1.06); }
  .line .char { color: var(--dim); }
  .line.past .char { color: var(--mid); }
  .line.active .char {
    background: linear-gradient(to right, var(--hl) 0%, var(--hl) var(--p, 0%), var(--dim) var(--p, 0%));
    -webkit-background-clip: text; background-clip: text;
    color: transparent; -webkit-text-fill-color: transparent;
  }
  body.mode-line .line.active { background: rgba(232,180,188,.10); border-radius: 12px;
    padding: 8px 14px; margin-left: -14px; }
  body.mode-line .line.active .char { background: none; -webkit-background-clip: initial;
    background-clip: initial; color: var(--fg); -webkit-text-fill-color: currentColor; }
  .bg-line { font-size: clamp(14px, 1.6vw, 20px); font-weight: 500; }
</style>
</head>
<body class="__MODE_CLS__">
  <div class="left">
    <div class="cover"><svg viewBox="0 0 24 24" fill="#8a7888"><path d="M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z"/></svg></div>
    <div class="meta"><div class="name">__TITLE__</div><div class="artist">__ARTIST__</div></div>
    <div class="controls">
      <span class="time" id="tCur">0:00</span>
      <div class="bar" id="bar"><div class="bar-track"><div class="bar-fill" id="barFill"></div></div><div class="bar-thumb" id="barThumb"></div></div>
      <span class="time" id="tEnd">0:00</span>
      <button id="play">▶</button>
    </div>
    <div class="track-switch">
      <button id="modeDuet" class="on">原唱</button>
      <button id="modeKaraoke">伴唱</button>
    </div>
    <div class="vol-row"><label>人声音量</label><input id="vocalVol" type="range" min="0" max="1" step="0.01" value="1"></div>
    <div class="vol-row"><label>伴奏音量</label><input id="instVol" type="range" min="0" max="1" step="0.01" value="1"></div>
    <div class="status" id="status">正在载入音频…</div>
    __MEDIA_ELEMS__
  </div>
  <div class="right"><div class="scroll" id="scroll"></div></div>
<script>
const MODE = '__MODE__';
const DATA = __DATA__;
const SRC_MODE = '__SRC_MODE__';   // data=内嵌 | link=外链相对路径
const INST_SRC = '__INST_URI__';   // 伴奏 data URI 或 相对文件名（主时钟）
const VOCAL_SRC = '__VOCAL_URI__'; // 人声 data URI 或 相对文件名（可能为空）
const TRACK_MODE = '__TRACK_MODE__'; // duet=原唱 | karaoke=伴唱
const scroll = document.getElementById('scroll');
const playBtn = document.getElementById('play');
const bar = document.getElementById('bar'), barFill = document.getElementById('barFill'), barThumb = document.getElementById('barThumb');
const tCur = document.getElementById('tCur'), tEnd = document.getElementById('tEnd');
const statusEl = document.getElementById('status');
const vocalVolEl = document.getElementById('vocalVol'), instVolEl = document.getElementById('instVol');
let els = [], activeIdx = -1, dragging = false;

// ---- Web Audio 单时钟双轨引擎（复刻 karaoke-main/ktv.js）----
// data 模式：两轨 base64 → decodeAudioData → 两个 BufferSource 同刻 start（采样级对齐）。
// link 模式：file:// 不能 fetch 相对路径，改用两个 <audio> + createMediaElementSource
//   接入同一个 AudioContext，仍由 GainNode 控制、切换只改 gain（播放头不动）。
let audioCtx = null, vocalsGain = null, instGain = null;
let vocalsBuffer = null, instBuffer = null;
let vocalsSource = null, instSource = null;
let instEl = null, vocalEl = null;          // link 模式的媒体元素
let audioStartTime = 0, audioOffset = 0, isPlaying = false;
let trackMode = TRACK_MODE, pausedByUs = false;

function initCtx() {
  if (!audioCtx) {
    const AC = window.AudioContext || window.webkitAudioContext;
    audioCtx = new AC();
    vocalsGain = audioCtx.createGain(); vocalsGain.connect(audioCtx.destination);
    instGain = audioCtx.createGain(); instGain.connect(audioCtx.destination);
    vocalsGain.gain.value = parseFloat(vocalVolEl.value);
    instGain.gain.value = parseFloat(instVolEl.value);
  }
  if (audioCtx.state === 'suspended') audioCtx.resume();
}
async function loadData(uri) {
  if (!uri) return null;
  const res = await fetch(uri);
  const arr = await res.arrayBuffer();
  return audioCtx.decodeAudioData(arr);
}
function initLinkElements() {
  instEl = document.getElementById('instEl');
  vocalEl = document.getElementById('vocalEl');
  if (instEl) {
    const n = audioCtx.createMediaElementSource(instEl);
    n.connect(instGain);
  }
  if (vocalEl) {
    const n = audioCtx.createMediaElementSource(vocalEl);
    n.connect(vocalsGain);
  }
}
function duration() {
  if (SRC_MODE === 'link') {
    return instEl && isFinite(instEl.duration) ? instEl.duration
      : (vocalEl && isFinite(vocalEl.duration) ? vocalEl.duration : 0);
  }
  return instBuffer ? instBuffer.duration : (vocalsBuffer ? vocalsBuffer.duration : 0);
}
function getPos() {
  if (!isPlaying) return audioOffset;
  if (SRC_MODE === 'link') return instEl ? instEl.currentTime : audioOffset;
  return audioOffset + (audioCtx.currentTime - audioStartTime);
}
function stopSources() {
  if (SRC_MODE === 'link') {
    if (instEl) instEl.pause();
    if (vocalEl) vocalEl.pause();
  } else {
    for (const s of [vocalsSource, instSource]) if (s) { try { s.stop(); } catch (e) {} }
    vocalsSource = null; instSource = null;
  }
  isPlaying = false;
}
function playAudio(offset = 0) {
  initCtx();
  stopSources();
  const dur = duration();
  offset = Math.max(0, Math.min(offset, dur ? dur - 0.01 : 0));
  audioStartTime = audioCtx.currentTime;
  audioOffset = offset;
  if (SRC_MODE === 'link') {
    // 两个媒体元素一起定位、一起播放，播放头由同一 JS 调用对齐
    if (instEl) { try { instEl.currentTime = offset; } catch (e) {} instEl.play().catch(() => {}); }
    if (vocalEl) { try { vocalEl.currentTime = offset; } catch (e) {} if (trackMode === 'duet') vocalEl.play().catch(() => {}); }
  } else {
    if (instBuffer) {
      instSource = audioCtx.createBufferSource(); instSource.buffer = instBuffer;
      instSource.connect(instGain); instSource.start(audioStartTime, offset);
    }
    if (vocalsBuffer) {
      vocalsSource = audioCtx.createBufferSource(); vocalsSource.buffer = vocalsBuffer;
      vocalsSource.connect(vocalsGain); vocalsSource.start(audioStartTime, offset);
    }
  }
  isPlaying = true;
}
function pauseAudio() { audioOffset = getPos(); stopSources(); }
function applyGains() {
  if (!audioCtx) return;
  const hasVocal = SRC_MODE === 'link' ? !!vocalEl : !!vocalsBuffer;
  vocalsGain.gain.value = (trackMode === 'duet' && hasVocal) ? parseFloat(vocalVolEl.value) : 0;
  instGain.gain.value = parseFloat(instVolEl.value);
  // link 模式切原唱：人声元素补播（gain 已归位，位置跟主元素对齐）
  if (SRC_MODE === 'link' && vocalEl && trackMode === 'duet' && isPlaying) {
    try { if (Math.abs(vocalEl.currentTime - instEl.currentTime) > 0.08) vocalEl.currentTime = instEl.currentTime; } catch (e) {}
    vocalEl.play().catch(() => {});
  }
}
function setTrack(mode) {
  trackMode = mode;
  document.getElementById('modeDuet').classList.toggle('on', mode === 'duet');
  document.getElementById('modeKaraoke').classList.toggle('on', mode === 'karaoke');
  applyGains();   // 只改增益，两轨播放头纹丝不动 → 绝无错位
}
document.getElementById('modeDuet').addEventListener('click', () => setTrack('duet'));
document.getElementById('modeKaraoke').addEventListener('click', () => setTrack('karaoke'));
vocalVolEl.addEventListener('input', applyGains);
instVolEl.addEventListener('input', applyGains);

playBtn.addEventListener('click', () => {
  if (isPlaying) { pauseAudio(); playBtn.textContent = '▶'; }
  else { playAudio(audioOffset); playBtn.textContent = '⏸'; }
});

DATA.lines.forEach((ln, i) => {
  const div = document.createElement('div');
  div.className = 'line' + (ln.bg ? ' bg-line' : ''); div.dataset.i = i;
  ln.words.forEach(w => { const c = document.createElement('span'); c.className = 'char'; c.textContent = w.char; div.appendChild(c); });
  div.addEventListener('click', () => { playAudio(ln.start / 1000); playBtn.textContent = '⏸'; });
  scroll.appendChild(div); els.push(div);
});
function fmt(s) { s = Math.max(0, s | 0); return (s / 60 | 0) + ':' + String(s % 60).padStart(2, '0'); }
function _fill(el, p) { el.style.setProperty('--p', Math.min(100, Math.max(0, p)).toFixed(1) + '%'); }
function tick() {
  const t = getPos() * 1000;
  if (!dragging) { tCur.textContent = fmt(getPos()); const d = duration(); if (d) _setBar(getPos() / d); }
  let idx = -1;
  DATA.lines.forEach((ln, i) => { if (t >= ln.start && t < ln.end + 300) idx = i; });
  els.forEach((el, i) => {
    el.classList.toggle('active', i === idx); el.classList.toggle('past', i < idx);
    if (i === idx && activeIdx !== idx) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    if (i === idx) {
      const chars = el.querySelectorAll('.char');
      if (MODE === 'line') chars.forEach(c => _fill(c, 100));
      else DATA.lines[i].words.forEach((w, k) => {
        const c = chars[k]; if (!c) return;
        if (t >= w.end) _fill(c, 100);
        else if (t >= w.start) _fill(c, (t - w.start) / Math.max(w.end - w.start, 1) * 100);
        else _fill(c, 0);
      });
    }
  });
  activeIdx = idx;
  requestAnimationFrame(tick);
}
function _setBar(r) { const p = Math.min(100, Math.max(0, r * 100)); barFill.style.width = p + '%'; barThumb.style.left = p + '%'; }
function _ratio(e) { const rect = bar.getBoundingClientRect(); return Math.min(1, Math.max(0, (e.clientX - rect.left) / Math.max(rect.width, 1))); }
bar.addEventListener('pointerdown', e => { if (!duration()) return; dragging = true; bar.classList.add('dragging'); bar.setPointerCapture(e.pointerId); const r = _ratio(e); _setBar(r); tCur.textContent = fmt(r * duration()); });
bar.addEventListener('pointermove', e => { if (!dragging) return; const r = _ratio(e); _setBar(r); tCur.textContent = fmt(r * duration()); });
bar.addEventListener('pointerup', e => { if (!dragging) return; dragging = false; bar.classList.remove('dragging'); const wasPlaying = isPlaying; playAudio(_ratio(e) * duration()); playBtn.textContent = wasPlaying ? '⏸' : '▶'; });
bar.addEventListener('pointercancel', () => { dragging = false; bar.classList.remove('dragging'); });

(async () => {
  requestAnimationFrame(tick);
  try {
    initCtx();
    if (SRC_MODE === 'link') {
      initLinkElements();
      // 等两轨元数据就绪才能读 duration / 定位
      await Promise.all([
        instEl ? new Promise(r => { if (instEl.readyState >= 1) return r(); instEl.addEventListener('loadedmetadata', r, { once: true }); }) : Promise.resolve(),
        vocalEl ? new Promise(r => { if (vocalEl.readyState >= 1) return r(); vocalEl.addEventListener('loadedmetadata', r, { once: true }); }) : Promise.resolve(),
      ]);
    } else {
      [instBuffer, vocalsBuffer] = await Promise.all([loadData(INST_SRC), loadData(VOCAL_SRC)]);
    }
    const d = duration();
    if (!d) { statusEl.textContent = '音频载入失败'; return; }
    tEnd.textContent = fmt(d);
    const hasVocal = SRC_MODE === 'link' ? !!vocalEl : !!vocalsBuffer;
    statusEl.textContent = hasVocal ? '双轨就绪 · 点播放开始' : '仅伴奏轨';
    applyGains();
  } catch (err) {
    statusEl.textContent = '音频解码失败：' + String(err);
  }
})();
</script>
</body>
</html>
"""


def _data_uri(audio_file: str | None) -> str:
    """音频 → base64 data URI（供 fetch(dataURI) 在 file:// 下解码）。"""
    if not audio_file:
        return ""
    src = Path(audio_file)
    if not src.is_file():
        return ""
    import base64
    ext = src.suffix.lstrip(".").lower()
    mime = {"mp3": "audio/mpeg", "wav": "audio/wav", "m4a": "audio/mp4",
            "flac": "audio/flac", "ogg": "audio/ogg"}.get(ext, "audio/mpeg")
    return f"data:{mime};base64," + base64.b64encode(src.read_bytes()).decode("ascii")


def _src_ref(audio_file: str | None) -> str:
    """外链模式：音频 → 同目录相对文件名（调用方已把音频复制进 player 所在目录）。"""
    if not audio_file:
        return ""
    src = Path(audio_file)
    return src.name if src.is_file() else ""


def build_duet_player_html(lyrics: Lyrics | None = None, *, lines_data: list | None = None,
                           instrumental_file: str | None, vocal_file: str | None,
                           title: str, artist: str, mode: str = "karaoke",
                           link: bool = False) -> str:
    """双轨 KTV 播放器（Web Audio 单时钟）：伴奏 + 人声，可切原唱/伴唱。

    link=False（默认，内嵌版）：两轨以 base64 data URI 内嵌为 JS 常量
      （本地 file:// 无法 fetch 相对路径，但可 fetch data: URI）。
    link=True（外链版）：两轨用同目录相对文件名，HTML 通过 <audio> +
      createMediaElementSource 接入同一 AudioContext（file:// 下可播）。
    两种模式都由 GainNode 控制、切换只改增益，零错位。
    缺人声轨时退化为只有伴奏（伴唱）的播放器。
    """
    if lines_data is not None:
        data = {"lines": lines_data}
    else:
        data = {
            "lines": [
                {
                    "start": ln.start,
                    "end": max(ln.end, ln.start + 200),
                    "bg": bool(ln.is_background),
                    "words": [{"char": w.char, "start": w.start, "end": w.end}
                              for w in ln.words],
                }
                for ln in (lyrics.lines if lyrics else [])
            ],
        }
    html = _DUET_TEMPLATE
    html = html.replace("__TITLE__", title or "Untitled")
    html = html.replace("__ARTIST__", artist or "")
    if link:
        inst_ref, vocal_ref = _src_ref(instrumental_file), _src_ref(vocal_file)
        html = html.replace("__SRC_MODE__", "link")
        media_elems = (f'<audio id="instEl" src="{inst_ref}" preload="auto"></audio>'
                       + (f'\n    <audio id="vocalEl" src="{vocal_ref}" preload="auto"></audio>'
                          if vocal_ref else ''))
        html = html.replace("__MEDIA_ELEMS__", media_elems)
    else:
        inst_ref, vocal_ref = _data_uri(instrumental_file), _data_uri(vocal_file)
        html = html.replace("__SRC_MODE__", "data")
        html = html.replace("__MEDIA_ELEMS__", "")
    html = html.replace("__INST_URI__", inst_ref)
    html = html.replace("__VOCAL_URI__", vocal_ref)
    html = html.replace("__DATA__", json.dumps(data, ensure_ascii=False))
    html = html.replace("__MODE__", "line" if mode == "line" else "karaoke")
    html = html.replace("__MODE_CLS__", "mode-line" if mode == "line" else "mode-karaoke")
    html = html.replace("__TRACK_MODE__", "duet" if vocal_file else "karaoke")
    return html


def save_duet_player(lyrics: Lyrics, instrumental_file: str | None, vocal_file: str | None,
                     out_path: Path, *, title: str = "", artist: str = "",
                     link: bool = False) -> Path:
    out_path = Path(out_path)
    out_path.write_text(
        build_duet_player_html(lyrics, instrumental_file=instrumental_file,
                               vocal_file=vocal_file, title=title, artist=artist,
                               link=link),
        encoding="utf-8")
    return out_path
