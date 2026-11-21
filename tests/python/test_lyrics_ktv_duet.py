# -*- coding: utf-8 -*-
"""双轨 KTV 播放器单测：原唱/伴唱切换 HTML + worker 导出（缺轨跳过）。"""
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python"))

from lyrics.player import build_duet_player_html, build_player_html
from lyrics.schema import Line, Lyrics, Word
from worker_lyrics import _export_ktv_duet_player, _find_instrumental_path


def _lyrics():
    return Lyrics(version=1, language="zh", source="test", alignment_model="x",
                  lines=[Line(id="L1", text="测试", start=0, end=2000,
                              words=[Word("测", 0, 1000, 0.9),
                                     Word("试", 1000, 2000, 0.9)])])


class DuetPlayerHtmlTests(unittest.TestCase):
    def test_two_tracks_embed_both_audio_and_sync_logic(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            vocal = root / "vocals.wav"
            inst = root / "instrumental.wav"
            vocal.write_bytes(b"RIFF" + b"\x00" * 64)
            inst.write_bytes(b"RIFF" + b"\x00" * 64)
            html = build_duet_player_html(_lyrics(), instrumental_file=str(inst),
                                          vocal_file=str(vocal), title="歌", artist="歌手")
        # Web Audio 单时钟：两轨以 base64 data URI 内嵌为 JS 常量（file:// 下可 fetch）
        self.assertIn('const INST_SRC = \'data:audio/wav;base64,', html)
        self.assertIn('const VOCAL_SRC = \'data:audio/wav;base64,', html)
        self.assertNotIn("__INST_URI__", html)
        self.assertNotIn("__VOCAL_URI__", html)
        self.assertNotIn("__MEDIA_ELEMS__", html)
        self.assertNotIn("__SRC_MODE__", html)
        # AudioContext + 两个 GainNode（人声/伴奏独立音量）+ decodeAudioData
        self.assertIn("new AC()", html)
        self.assertIn("decodeAudioData", html)
        self.assertIn("createBufferSource", html)
        self.assertIn("createGain", html)
        self.assertIn('id="vocalVol"', html)
        self.assertIn('id="instVol"', html)
        # 两轨同一 startTime/offset 启动（单时钟，采样级对齐）
        self.assertIn("vocalsSource.start(audioStartTime, offset)", html)
        self.assertIn("instSource.start(audioStartTime, offset)", html)
        self.assertIn("audioStartTime = audioCtx.currentTime", html)
        # 原唱/伴唱切换只改 GainNode.gain，播放头不动（零错位根因）
        set_track_block = html.split("function setTrack(mode) {", 1)[1].split(
            "document.getElementById('modeDuet').addEventListener", 1)[0]
        self.assertIn("applyGains();", set_track_block)
        self.assertNotIn("playAudio", set_track_block)
        self.assertNotIn("stopSources", set_track_block)
        self.assertIn("const TRACK_MODE = 'duet'", html)
        # 位置靠推算（非读两个 audio 的 currentTime）
        self.assertIn("function getPos()", html)
        self.assertIn("audioOffset + (audioCtx.currentTime - audioStartTime)", html)
        # 歌词数据完整注入（逐字 JSON，中文原文保留）
        self.assertIn('"char": "测"', html)
        self.assertIn('"char": "试"', html)

    def test_no_vocal_track_degrades_to_karaoke_only(self):
        with tempfile.TemporaryDirectory() as td:
            inst = Path(td) / "instrumental.mp3"
            inst.write_bytes(b"ID3" + b"\x00" * 32)
            html = build_duet_player_html(_lyrics(), instrumental_file=str(inst),
                                          vocal_file=None, title="歌", artist="")
        self.assertIn('const INST_SRC = \'data:audio/mpeg;base64,', html)
        self.assertIn("const VOCAL_SRC = ''", html)
        self.assertIn('const TRACK_MODE = \'karaoke\'', html)

    def test_data_uri_roundtrip(self):
        # 内嵌音频解码后应与源文件字节一致
        import base64
        import re
        with tempfile.TemporaryDirectory() as td:
            vocal = Path(td) / "vocals.wav"
            payload = b"RIFF" + bytes(range(64))
            vocal.write_bytes(payload)
            html = build_duet_player_html(_lyrics(), instrumental_file=None,
                                          vocal_file=str(vocal), title="t", artist="")
        m = re.search(r"const VOCAL_SRC = 'data:audio/wav;base64,([^']+)'", html)
        self.assertIsNotNone(m)
        self.assertEqual(base64.b64decode(m.group(1)), payload)

    def test_original_single_track_template_untouched(self):
        html = build_player_html(_lyrics(), audio_file=None, title="歌", artist="")
        self.assertNotIn("vocalAudio", html)
        self.assertNotIn("modeDuet", html)
        self.assertIn('id="audio"', html)

    def test_single_track_still_inlines_base64(self):
        # 共用 _audio_tag 加了 inline 开关；单轨播放器仍应保持 base64 内嵌行为
        with tempfile.TemporaryDirectory() as td:
            src = Path(td) / "song.mp3"
            src.write_bytes(b"ID3" + b"\x00" * 32)
            html = build_player_html(_lyrics(), audio_file=str(src), title="歌", artist="")
        self.assertIn("data:audio/mpeg;base64,", html)


class WorkerDuetExportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.out = self.root / "song"
        self.out.mkdir()
        self.vocal = self.root / "01_vocals.mp3"
        self.inst = self.root / "02_instrumental.mp3"
        self.vocal.write_bytes(b"ID3" + b"\x00" * 128)
        self.inst.write_bytes(b"ID3" + b"\x00" * 128)
        self.stages = []

        # worker_lyrics 顶部已 from worker_protocol import emit，须 patch 本模块引用
        def fake_emit(event, payload=None, **kwargs):
            if event == "task_stage":
                self.stages.append((payload or {}).get("message", ""))

        self.emit_patch = mock.patch("worker_lyrics.emit", fake_emit)
        self.emit_patch.start()
        self.addCleanup(self.emit_patch.stop)

    def test_both_tracks_produce_default_ktv_player_dir(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "artist": "歌手"}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t1")
        self.assertIn("ktvDuetPlayer", result)
        self.assertIn("ktvDuetLinkPlayer", result)
        self.assertNotIn("ktvDuetEvenPlayer", result)  # evenSpread 未开
        embed = Path(result["ktvDuetPlayer"])
        self.assertEqual(embed.parent.name, "ktv-player")
        self.assertEqual(embed.name, "song.ktv.player.html")
        duet_dir = embed.parent
        self.assertTrue((duet_dir / "vocals.mp3").is_file())
        self.assertTrue((duet_dir / "instrumental.mp3").is_file())
        html = embed.read_text(encoding="utf-8")
        self.assertIn("const VOCAL_SRC = 'data:audio/mpeg;base64,", html)
        self.assertIn("decodeAudioData", html)
        self.assertIn("原唱", html)
        # 外链版：SRC_MODE=link，引用相对文件名，不内嵌 base64
        link = Path(result["ktvDuetLinkPlayer"])
        self.assertEqual(link.name, "song.ktv.link.player.html")
        link_html = link.read_text(encoding="utf-8")
        self.assertIn("const SRC_MODE = 'link'", link_html)
        self.assertIn('id="instEl" src="instrumental.mp3"', link_html)
        self.assertNotIn("data:audio/", link_html)
        self.assertTrue(any("已导出双轨 KTV 播放器" in m for m in self.stages))

    def test_custom_ktv_dir_and_even_version(self):
        # 指定专用目录 + 均摊模式 → <root>/<stem>/ 下三版本 + 两轨音频
        root = self.root / "ktv_root"
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetDir": str(root), "evenSpread": True}
        result = _export_ktv_duet_player(_lyrics(), self.out, "晴天", payload, "t1b")
        self.assertEqual(set(result), {"ktvDuetPlayer", "ktvDuetLinkPlayer", "ktvDuetEvenPlayer"})
        song_dir = root / "晴天"
        self.assertTrue(song_dir.is_dir())
        for name in ["晴天.ktv.player.html", "晴天.ktv.link.player.html",
                     "晴天.ktv.even.player.html", "vocals.mp3", "instrumental.mp3"]:
            self.assertTrue((song_dir / name).is_file(), name)
        even_html = (song_dir / "晴天.ktv.even.player.html").read_text(encoding="utf-8")
        # 标题只用原名，不带「原唱/伴唱」「均摊」后缀（版本靠文件名区分）
        self.assertIn("<title>晴天</title>", even_html)
        self.assertNotIn("原唱/伴唱", even_html)
        self.assertNotIn("· 均摊", even_html)
        self.assertIn("const SRC_MODE = 'data'", even_html)

    def test_select_only_embed_variant(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["embed"]}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t5")
        self.assertEqual(set(result), {"ktvDuetPlayer"})
        duet_dir = Path(result["ktvDuetPlayer"]).parent
        self.assertFalse((duet_dir / "song.ktv.link.player.html").exists())
        self.assertFalse((duet_dir / "song.ktv.even.player.html").exists())
        self.assertTrue(any("（内嵌）" in m for m in self.stages))

    def test_select_link_and_even_variants(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["link", "even"], "evenSpread": True}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t6")
        self.assertEqual(set(result), {"ktvDuetLinkPlayer", "ktvDuetEvenPlayer"})
        duet_dir = Path(result["ktvDuetLinkPlayer"]).parent
        self.assertFalse((duet_dir / "song.ktv.player.html").exists())

    def test_even_variant_requires_even_spread(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["even"]}  # evenSpread 未开
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t7")
        self.assertEqual(result, {})
        self.assertFalse((self.out / "ktv-player").exists())
        self.assertTrue(any("均摊版需开启均摊估计模式" in m for m in self.stages))

    def test_select_even_link_variant(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["evenLink"], "evenSpread": True}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t9")
        self.assertEqual(set(result), {"ktvDuetEvenLinkPlayer"})
        link = Path(result["ktvDuetEvenLinkPlayer"])
        self.assertEqual(link.name, "song.ktv.even.link.player.html")
        html = link.read_text(encoding="utf-8")
        self.assertIn("const SRC_MODE = 'link'", html)
        self.assertIn('id="instEl" src="instrumental.mp3"', html)
        self.assertNotIn("data:audio/", html)
        self.assertIn("<title>song</title>", html)
        self.assertFalse((link.parent / "song.ktv.even.player.html").exists())
        self.assertTrue(any("（均摊外链）" in m for m in self.stages))

    def test_select_both_even_variants(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["even", "evenLink"], "evenSpread": True}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t10")
        self.assertEqual(set(result), {"ktvDuetEvenPlayer", "ktvDuetEvenLinkPlayer"})
        embed = Path(result["ktvDuetEvenPlayer"])
        self.assertIn("const SRC_MODE = 'data'", embed.read_text(encoding="utf-8"))
        self.assertIn("均摊内嵌", " ".join(self.stages))

    def test_even_link_variant_requires_even_spread(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": ["evenLink"]}  # evenSpread 未开
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t11")
        self.assertEqual(result, {})
        self.assertFalse((self.out / "ktv-player").exists())

    def test_empty_variants_skips(self):
        payload = {"vocalsPath": str(self.vocal), "instrumentalPath": str(self.inst),
                   "ktvDuetVariants": []}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t8")
        self.assertEqual(result, {})
        self.assertFalse((self.out / "ktv-player").exists())
        self.assertTrue(any("未勾选任何播放器版本" in m for m in self.stages))

    def test_missing_instrumental_skips_without_output(self):
        # 人声放在独立目录，旁边没有伴奏 → 派生不到 → 跳过
        isolated = self.root / "vocal_only"
        isolated.mkdir()
        v = isolated / "vocals.mp3"
        v.write_bytes(b"ID3" + b"\x00" * 16)
        payload = {"vocalsPath": str(v), "instrumentalPath": ""}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t2")
        self.assertEqual(result, {})
        self.assertFalse((self.out / "ktv-player").exists())
        self.assertTrue(any("缺少伴奏轨" in m for m in self.stages))

    def test_missing_vocals_skips_without_output(self):
        payload = {"vocalsPath": "/no/such/vocals.mp3",
                   "instrumentalPath": str(self.inst)}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t3")
        self.assertEqual(result, {})
        self.assertFalse((self.out / "ktv-player").exists())
        self.assertTrue(any("缺少人声轨" in m for m in self.stages))

    def test_nonexistent_instrumental_path_falls_back_to_sibling(self):
        # 显式路径无效时回退到人声同目录按文件名匹配到伴奏
        payload = {"vocalsPath": str(self.vocal),
                   "instrumentalPath": "/no/such/instrumental.mp3"}
        result = _export_ktv_duet_player(_lyrics(), self.out, "song", payload, "t4")
        self.assertIn("ktvDuetPlayer", result)
        self.assertTrue(Path(result["ktvDuetPlayer"]).is_file())

    def test_find_instrumental_by_filename(self):
        found = _find_instrumental_path(self.vocal)
        self.assertEqual(found, self.inst)

    def test_find_instrumental_ignores_vocal_and_unrelated(self):
        unrelated = self.root / "cover.jpg"
        unrelated.write_bytes(b"jpg")
        found = _find_instrumental_path(self.vocal)
        self.assertEqual(found, self.inst)

    def test_find_instrumental_in_sibling_folder(self):
        # 分层布局：separated/Vocals/x.mp3 + separated/Instrumental/x.mp3
        base = self.root / "layered"
        vdir = base / "Vocals"
        idir = base / "Instrumental"
        vdir.mkdir(parents=True)
        idir.mkdir(parents=True)
        v = vdir / "01_song_Vocals.mp3"
        inst = idir / "02_song_Instrumental.mp3"
        v.write_bytes(b"ID3")
        inst.write_bytes(b"ID3")
        self.assertEqual(_find_instrumental_path(v), inst)

    def test_find_instrumental_none_when_only_vocal(self):
        only = self.root / "only_vocal_dir"
        only.mkdir()
        v = only / "vocals.mp3"
        v.write_bytes(b"ID3")
        self.assertIsNone(_find_instrumental_path(v))


if __name__ == "__main__":
    unittest.main()
