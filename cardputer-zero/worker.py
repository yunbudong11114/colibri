"""Cardputer Zero session and local UI protocol."""
import argparse
from array import array
import asyncio
from contextlib import asynccontextmanager
from dataclasses import asdict, dataclass, replace
from functools import lru_cache
from hashlib import sha256
import fcntl
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import wave

from urllib.parse import urlsplit

from pypinyin import Style, pinyin
from pypinyin.constants import PINYIN_DICT

from colibri.config import AgentConfig, ConfigError, VisionConfig
from colibri.memory import ALWAYS_ON_MEMORY_FILES
from colibri.model.factory import build_model_client
from colibri.model.openai_compatible import OpenAICompatibleModelClient
from colibri.paths import colibri_home
from colibri.session import AgentSession
from colibri.session_history import TranscriptHistoryLoader
from colibri.skills import SkillIndex
from colibri.tools.base import ToolSpec, ToolResult, bound_tool_text
from colibri.tools.builtin import (
    HARDWARE_TOOLS, FilesListTool, FilesReadTool, FilesSendTool, FilesWriteTool,
    ImageUnderstandTool, ShellRunTool,
)
from colibri.tools.builtin.memory import (
    MemoryListTool, MemoryReadTool, MemorySearchTool, MemoryWriteTool, _memory_file_path,
)
from colibri.tools.builtin.skills import SkillReadTool, SkillRunTool
from colibri.tools.builtin.web import WebSearchTool
from colibri.tools.permissions import PermissionDecisionResult
from colibri.tools.registry import ToolRegistry
from colibri.transcript import TranscriptWriter, beijing_date


MEMORY_FILES = {*ALWAYS_ON_MEMORY_FILES, "INDEX.md"}
MEMORY_EDITOR_MAX_BYTES = 64 * 1024


MODEL_KEY_HOSTS = {"api.openai.com", "api.deepseek.com", "openrouter.ai"}
NCNN_MODEL_FILES = (
    "tokens.txt",
    "encoder.ncnn.param", "encoder.ncnn.bin",
    "decoder.ncnn.param", "decoder.ncnn.bin",
    "joiner.ncnn.param", "joiner.ncnn.bin",
)
NAS_STORAGE_NAME = re.compile(
    r"(同步到|备份到|保存到|上传到|传到|写入|挂载到|连接到)\s*纳斯(?=$|[\s，。！？,.!?])"
)


VISION_FIELDS = {f"vision.{key}": value for key, value in asdict(VisionConfig()).items()}


FIELDS = {
    **VISION_FIELDS,
    "model.provider": "openai_compatible",
    "model.base_url": "https://api.openai.com/v1",
    "model.model": "gpt-4.1-mini",
    "model.api_key": "",
    "speech.backend": "auto",
    "speech.base_url": "",
    "speech.model": "whisper-1",
    "speech.api_key": "",
    "web_search.engine": "baidu",
    "web_search.endpoint": "https://qianfan.baidubce.com/v2/ai_search/web_search",
    "web_search.api_key": "",
    "session.transcript": True,
    "session.restore_transcript": True,
    "session.transcript_retention_days": 30,
    "session.transcript_max_total_bytes": 32 * 1024 * 1024,
    "memory.enabled": True,
    "memory.root": "~/.colibri/memory",
    "mcp.servers": [],
}


class Settings:
    def __init__(self, home):
        self.home = home
        self.path = home / "settings.json"
        self.values = self.read()

    def read(self):
        values = {key: value for key, value in FIELDS.items() if key not in VISION_FIELDS}
        if self.path.exists():
            stored = json.loads(self.path.read_text(encoding="utf-8"))
            values.update({key: stored[key] for key in FIELDS if key in stored})
        validate_servers(values["mcp.servers"])
        return values

    def save(self, key, raw):
        default = FIELDS[key]
        value = raw if isinstance(default, str) else json.loads(raw)
        if type(value) is not type(default) or isinstance(value, int) and value < 0:
            raise ValueError("设置值类型或范围错误")
        if key in VISION_FIELDS and isinstance(value, int) and value == 0:
            raise ValueError("请输入大于 0 的整数")
        if key in VISION_FIELDS and isinstance(value, str):
            value = value.strip()
        if key.endswith(".base_url") or key.endswith(".endpoint"):
            url = urlsplit(value)
            if (value or key not in {"speech.base_url", "vision.base_url"}) and (url.scheme not in {"http", "https"} or not url.hostname or url.username):
                raise ValueError("请输入 HTTP(S) URL")
        if key == "model.provider" and value not in {"fake", "openai_compatible"}:
            raise ValueError("支持 fake 和 openai_compatible")
        if key == "speech.backend" and value not in {"auto", "offline", "online"}:
            raise ValueError("支持 auto（自动）、offline（本机离线）和 online（在线 API）")
        if key == "web_search.engine" and value not in {"baidu", "aliyun_mcp"}:
            raise ValueError("支持 baidu 和 aliyun_mcp")
        if key == "memory.root":
            value = value.strip()
            if not Path(value).expanduser().is_absolute():
                raise ValueError("请输入绝对路径或 ~/ 开头的目录")
        if key == "mcp.servers":
            validate_servers(value)
        self.update(lambda _: {key: value})

    def save_model(self, base_url, model, api_key):
        base_url = base_url.strip().rstrip("/").removesuffix("/chat/completions")
        url = urlsplit(base_url)
        if url.scheme not in {"http", "https"} or not url.hostname or url.username or url.query or url.fragment:
            raise ValueError("请输入 HTTP(S) Base URL，地址不含账号、查询参数或片段")
        if not model.strip():
            raise ValueError("请输入模型 ID")

        def changes(previous):
            key = api_key.strip() if api_key is not None else ""
            if api_key is None and previous["model.base_url"].rstrip("/") == base_url:
                key = previous["model.api_key"]
            if url.hostname in MODEL_KEY_HOSTS and not key:
                raise ValueError("请填写这个服务商的 API Key")
            return {"model.base_url": base_url, "model.model": model.strip(),
                    "model.api_key": key, "model.provider": "openai_compatible"}

        self.update(changes)

    def update(self, changes):
        self.home.mkdir(parents=True, exist_ok=True)
        with (self.home / "settings.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            previous = self.read()
            updated = previous | changes(previous)
            fd, path = tempfile.mkstemp(dir=self.home)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as stream:
                    json.dump(updated, stream, ensure_ascii=False, indent=2)
                os.replace(path, self.path)
            finally:
                Path(path).unlink(missing_ok=True)
            self.values = updated

    def toml_config(self):
        path = self.home / "config.toml"
        return tomllib.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}

    def public(self):
        fields = []
        vision = self.toml_config().get("vision", {})
        values = VISION_FIELDS | {key: vision[key.removeprefix("vision.")]
                                  for key in VISION_FIELDS if key.removeprefix("vision.") in vision}
        for key, value in (values | self.values).items():
            secret = key.endswith("api_key") or key == "mcp.servers"
            display = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
            fields.append({"key": key, "value": ("•••" if value else "") if secret else display,
                           "secret": secret})
        return fields

    def agent_config(self):
        stored = self.toml_config()
        overrides = {group: stored[group] for group in ("files", "shell", "vision", "hardware")
                     if group in stored}
        files = overrides.setdefault("files", {})
        files["roots"] = [self.home, *files.get("roots", AgentConfig.default().files.roots)]
        overrides.setdefault("shell", {}).setdefault("deny", [])
        overrides["skills"] = {"dir": self.home / "skills"}
        for key, value in self.values.items():
            group, field = key.split(".", 1)
            if group in {"model", "vision", "web_search", "session", "memory"}:
                overrides.setdefault(group, {})[field] = value
        return AgentConfig.default().with_overrides(overrides)


def validate_servers(servers):
    if not isinstance(servers, list) or len(servers) > 8:
        raise ValueError("MCP 配置需要服务器数组，最多 8 个")
    seen = set()
    for server in servers:
        if not isinstance(server, dict):
            raise ValueError("MCP 服务器必须是对象")
        name = server.get("name", "")
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,16}", name) or name in seen:
            raise ValueError("MCP 名称重复或格式错误")
        seen.add(name)
        if ("url" in server) == ("command" in server):
            raise ValueError("MCP 服务器需要 url 或 command")
        if "description" in server and not isinstance(server["description"], str):
            raise ValueError("MCP description 需要字符串")
        if "token" in server and not isinstance(server["token"], str):
            raise ValueError("MCP token 需要字符串")
        if "url" in server:
            if not isinstance(server["url"], str):
                raise ValueError("MCP URL 格式错误")
            url = urlsplit(server["url"])
            if url.scheme not in {"http", "https"} or not url.hostname or url.username:
                raise ValueError("MCP URL 格式错误")
        if "command" in server and (not isinstance(server["command"], str) or not server["command"]):
            raise ValueError("MCP command 格式错误")
        env = server.get("env", {})
        if not isinstance(env, dict) or any(not isinstance(value, str) for value in env.values()):
            raise ValueError("MCP env 需要字符串键值对")
        for key in ("args", "read_only"):
            values = server.get(key, [])
            if not isinstance(values, list) or any(not isinstance(value, str) for value in values):
                raise ValueError(f"MCP {key} 需要字符串数组")


@asynccontextmanager
async def connect(server):
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    from mcp.client.streamable_http import streamable_http_client
    import httpx

    async with asyncio.timeout(30):
        if "command" in server:
            params = StdioServerParameters(command=server["command"], args=server.get("args", []),
                                           env=server.get("env"))
            async with stdio_client(params) as (read, write), ClientSession(read, write) as session:
                await session.initialize()
                yield session
        else:
            token = server.get("token")
            headers = {"Authorization": f"Bearer {token}"} if token else {}
            async with httpx.AsyncClient(headers=headers, timeout=20) as http:
                async with streamable_http_client(server["url"], http_client=http) as (read, write, _):
                    async with ClientSession(read, write) as session:
                        await session.initialize()
                        yield session


async def mcp_request(server, operation, name=None, arguments=None):
    async with connect(server) as client:
        if operation == "call_tool":
            return await client.call_tool(name, arguments)
        if operation == "read_resource":
            from pydantic import AnyUrl
            return await client.read_resource(AnyUrl(name))
        if operation == "get_prompt":
            return await client.get_prompt(name, arguments)
        results = []
        field = operation.removeprefix("list_")
        cursor = None
        for _ in range(20):
            page = await getattr(client, operation)(cursor=cursor)
            results.extend(getattr(page, field))
            cursor = page.nextCursor
            if cursor is None:
                return results
        raise ValueError("MCP 列表超过分页上限")


@dataclass
class MCPTool:
    spec: ToolSpec
    server: dict
    remote_name: str

    def run(self, arguments, context):
        try:
            result = asyncio.run(mcp_request(self.server, "call_tool", self.remote_name, arguments))
            response = result.structuredContent
            if response is None:
                response = [part.text for part in result.content if part.type == "text"]
            text, truncated = bound_tool_text(
                json.dumps(response, ensure_ascii=False), context.config.tools.max_result_chars
            )
            return ToolResult(not result.isError, text, "mcp_error" if result.isError else None, truncated)
        except Exception as error:
            return ToolResult(False, f"MCP 调用失败 ({type(error).__name__})", "mcp_error")


class CardputerMemoryWriteTool(MemoryWriteTool):
    def run(self, arguments, context):
        target = _memory_file_path(arguments, context)
        if target is not None:
            path, label = target
            root = context.config.memory.root.expanduser().resolve()
            if path.resolve() != root / label or path.is_symlink() or (
                label.startswith("topics/") and path.parent.is_symlink()
            ):
                return ToolResult(ok=False, text="记忆写入不支持符号链接", error_type="invalid_arguments")
        return super().run(arguments, context)


class CardputerToolPolicy:
    @staticmethod
    def decide(tool, arguments, context):
        return PermissionDecisionResult(True, "allow", "default")


class CardputerShellTool(ShellRunTool):
    spec = replace(ShellRunTool.spec, description="Run a Linux shell command on the Cardputer.")


@lru_cache(maxsize=1)
def pinyin_index():
    entries = {}
    common = "的一是不了在人有我他这中大来上国个到说们为子和你地出道也时年得就那要下以生会自着去之过家学对可她里后小么心多天而能好都然没日于起还发成事只作当想看文无开手十用主行方又如前所本见经头面公同三已老从动两长知民样现分将点明目因问意最回新代向情部正名给次世再些外声水做高什月真话很听第打吃喝谢早晚零鸟卡"
    for char in common + "".join(chr(code) for code in PINYIN_DICT):
        for syllable in pinyin(char, style=Style.NORMAL, heteronym=True)[0]:
            bucket = entries.setdefault(syllable, {})
            bucket[char] = None
    return entries


def pinyin_candidates(spelling):
    return "".join(pinyin_index().get(spelling, {}))


class App:
    def __init__(self, home):
        self.config = Settings(home)
        self.session = None
        self.recording = None

    def close(self):
        self.finish_voice(cancel=True)
        if self.session:
            self.session.close()
            self.session = None

    def chat_model(self, model_config):
        if model_config.provider == "openai_compatible":
            try:
                url = urlsplit(model_config.base_url)
            except ValueError as error:
                raise ConfigError("Invalid model URL") from error
            if url.scheme not in {"http", "https"} or not url.hostname or not model_config.model.strip():
                raise ConfigError("Invalid model connection")
            if url.hostname not in MODEL_KEY_HOSTS:
                return OpenAICompatibleModelClient(
                    model_config.base_url.rstrip("/"), model_config.model, model_config.api_key,
                    model_config.max_retries, model_config.retry_backoff_ms,
                )
        return build_model_client(model_config)

    def model_hint(self):
        try:
            self.chat_model(self.config.agent_config().model)
        except ConfigError as error:
            return error_message(error)
        return ""

    def setup(self, restore=True):
        config = self.config.agent_config()
        model = self.chat_model(config.model)
        self.session = AgentSession(config=config, model=model,
                                    permission_policy=CardputerToolPolicy())
        retention = config.session
        home = self.config.home
        if retention.transcript:
            self.session.transcript = self.open_transcript(retention)
        if restore and retention.transcript and retention.restore_transcript:
            self.session.history_loader = TranscriptHistoryLoader(
                home, message_limit=retention.restore_message_limit,
                char_limit=retention.restore_char_limit, scan_bytes=retention.restore_scan_bytes,
            )

    def open_transcript(self, retention):
        path = self.config.home / "transcripts" / f"{beijing_date()}-{time.time_ns()}.jsonl"
        return TranscriptWriter(path, retention_days=retention.transcript_retention_days,
                                max_total_bytes=retention.transcript_max_total_bytes)

    def ask(self, prompt, status):
        if self.session is None:
            self.setup()
        if self.session.tools is None:
            tools = [
                FilesListTool(), FilesReadTool(), FilesWriteTool(), FilesSendTool(),
                CardputerShellTool(), ImageUnderstandTool(), SkillReadTool(), SkillRunTool(),
                WebSearchTool(), *(tool() for tool in HARDWARE_TOOLS),
            ]
            if self.session.config.memory.enabled:
                tools.extend((MemoryListTool(), MemoryReadTool(), MemorySearchTool(),
                              CardputerMemoryWriteTool()))
            for server in self.config.values["mcp.servers"]:
                try:
                    for tool in asyncio.run(mcp_request(server, "list_tools")):
                        name = f"mcp.{server['name']}.{tool.name}"
                        spec = ToolSpec(name, tool.description or tool.name, tool.inputSchema)
                        tools.append(MCPTool(spec, server, tool.name))
                except Exception as error:
                    status(f"MCP {server['name']} 连接失败 ({type(error).__name__})")
            self.session.tools = ToolRegistry(tools, cwd=self.config.home)
        answer = self.session.submit(prompt).text
        transcript = self.session.transcript
        if isinstance(transcript, TranscriptWriter):
            retention = self.session.config.session
            cap = retention.transcript_max_total_bytes
            rotate = not transcript.path.name.startswith(beijing_date())
            if cap:
                try:
                    rotate |= transcript.path.stat().st_size >= min(cap, 1024 * 1024)
                except FileNotFoundError:
                    rotate = True
            if rotate:
                transcript.close()
                self.session.transcript = self.open_transcript(retention)
        return answer

    def save(self, key, value):
        self.config.save(key, value)
        self.reload_session()

    def reload_session(self):
        # Keep the active conversation across configuration changes.
        old = self.session
        self.close()
        if old:
            self.setup(restore=False)
            self.session.messages = old.messages
            self.session.summary = old.summary

    def catalog(self, kind):
        if kind == "skills":
            return [{"name": item.name, "description": item.description}
                    for item in SkillIndex.scan(self.config.home / "skills").skills]
        if kind == "mcp":
            return [
                {"name": server["name"], "description": server.get("description") or
                 ("stdio · 未填写说明" if "command" in server else "Streamable HTTP · 未填写说明")}
                for server in self.config.values["mcp.servers"]
            ]
        raise ValueError("列表类型无效")

    def memory_path(self, name):
        if name not in MEMORY_FILES and not re.fullmatch(r"topics/[A-Za-z0-9_-]+\.md", name):
            raise ValueError("记忆文件名无效")
        root = self.config.agent_config().memory.root.resolve()
        path = root / name
        if path.resolve() != path:
            raise ValueError("记忆编辑器无法打开符号链接")
        return path

    def memory_topics(self):
        root = self.config.agent_config().memory.root.resolve()
        folder = root / "topics"
        if folder.resolve() != folder:
            return []
        return sorted(f"topics/{p.name}" for p in folder.glob("*.md")
                      if p.is_file() and not p.is_symlink() and re.fullmatch(r"[A-Za-z0-9_-]+", p.stem))

    def read_memory(self, name):
        path = self.memory_path(name)
        try:
            with path.open("rb") as stream:
                data = stream.read(MEMORY_EDITOR_MAX_BYTES + 1)
        except FileNotFoundError:
            return {"file": name, "text": "", "revision": None}
        if len(data) > MEMORY_EDITOR_MAX_BYTES:
            raise ValueError("文件超过 64 KiB，请在设备终端编辑")
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError("记忆文件需要 UTF-8 编码") from error
        if "\0" in text:
            raise ValueError("记忆文件包含不能显示的空字节")
        return {"file": name, "text": text, "revision": sha256(data).hexdigest()}

    def save_memory(self, name, text, revision):
        payload = text.encode("utf-8")
        if len(payload) > MEMORY_EDITOR_MAX_BYTES or "\0" in text:
            raise ValueError("文件内容无效或超过 64 KiB")
        path = self.memory_path(name)
        lock_path = self.config.home / "settings.lock"
        lock_path.parent.mkdir(parents=True, exist_ok=True)
        with lock_path.open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if self.read_memory(name)["revision"] != revision:
                raise ValueError("文件已在别处修改，请重新打开")
            path.parent.mkdir(parents=True, exist_ok=True)
            fd, temporary = tempfile.mkstemp(dir=path.parent)
            try:
                with os.fdopen(fd, "wb") as stream:
                    stream.write(payload)
                os.replace(temporary, path)
            finally:
                Path(temporary).unlink(missing_ok=True)
        return {"file": name, "text": text, "revision": sha256(payload).hexdigest()}

    def start_voice(self):
        if self.recording:
            return
        offline = self.offline_speech()
        if not offline:
            self.speech_endpoint()
        temp = tempfile.TemporaryDirectory(prefix="colibri-voice-")
        path = Path(temp.name) / "speech.wav"
        try:
            command, environment = self.capture_command(path)
            process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL)
        except BaseException:
            temp.cleanup()
            raise
        self.recording = (temp, path, process)
        if command[0] == "timeout":
            try:
                self.wait_for_microphone(path, process)
            except BaseException:
                self.finish_voice(cancel=True)
                raise
        return "offline" if offline else "online"

    @staticmethod
    def wait_for_microphone(path, process):
        deadline = time.monotonic() + 5
        while process.poll() is None and time.monotonic() < deadline:
            try:
                if path.stat().st_size > 44:
                    return
            except FileNotFoundError:
                pass
            time.sleep(0.05)
        raise ValueError("麦克风未开始采样，请检查系统音频输入。")

    def capture_command(self, path):
        device = os.getenv("COLIBRI_CAPTURE_DEVICE")
        board_audio = os.getenv("COLIBRI_DEVICE_AUDIO") == "1"
        runtime = Path(os.getenv("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
        if board_audio and not device and (runtime / "pulse/native").is_socket() and shutil.which("parecord"):
            return [
                "timeout", "--foreground", "--signal=INT", "--kill-after=3s", "60s",
                "parecord", "--file-format=wav", "--rate=16000", "--channels=2",
                "--format=s16le", "--latency-msec=50",
                "--device=alsa_input.platform-sound.stereo-fallback", str(path),
            ], os.environ | {"XDG_RUNTIME_DIR": str(runtime)}
        device = device or ("plughw:CARD=ES8389Audio,DEV=0" if board_audio else "default")
        return ["arecord", "-q", "-D", device, "-t", "wav", "-f", "S16_LE",
                "-r", "16000", "-c", "1", "-d", "60", str(path)], None

    def finish_voice(self, cancel=False):
        if self.recording is None:
            return ""
        temp, path, process = self.recording
        self.recording = None
        try:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            if cancel:
                return ""
            try:
                with wave.open(str(path), "rb") as audio:
                    params = audio.getparams()
                    frames = audio.readframes(min(audio.getnframes(), 60 * audio.getframerate()))
                frame_size = params.nchannels * params.sampwidth
                frames = frames[:len(frames) // frame_size * frame_size]
                if not frames:
                    raise ValueError("录音为空，请重试")
                if params.nchannels == 2 and params.sampwidth == 2:
                    mono = array("h", frames)[::2]
                    peak = max(map(abs, mono))
                    if 0 < peak < 22000:
                        gain = min(8, 22000 / peak)
                        mono = array("h", (round(sample * gain) for sample in mono))
                    frames = mono.tobytes()
                    params = params._replace(nchannels=1)
                normalized = Path(temp.name) / "finished.wav"
                with wave.open(str(normalized), "wb") as audio:
                    audio.setparams(params._replace(nframes=0))
                    audio.writeframes(frames)
            except (FileNotFoundError, EOFError, wave.Error) as error:
                raise ValueError("麦克风录音失败，请检查系统音频输入") from error
            return self.transcribe(normalized)
        finally:
            temp.cleanup()

    def offline_speech(self):
        backend = self.config.values["speech.backend"]
        if backend == "online" or (backend == "auto" and self.config.values["speech.base_url"]):
            return None
        root = Path(os.getenv("COLIBRI_OFFLINE_SPEECH", Path(__file__).parent / "offline-speech"))
        if all((root / name).is_file() for name in ("sherpa-ncnn", *NCNN_MODEL_FILES)):
            return root
        if all((root / name).is_file() for name in ("sherpa-onnx", "model.int8.onnx", "tokens.txt")):
            return root
        if backend == "offline":
            raise ValueError("离线语音模型未安装，请安装包含语音模型的 colibri 包。")
        return None

    def speech_endpoint(self):
        config = self.config.values
        url = config["speech.base_url"] or config["model.base_url"]
        key = config["speech.api_key"]
        if not key and not config["speech.base_url"]:
            key = config["model.api_key"] or os.getenv("COLIBRI_API_KEY", "")
        if urlsplit(url).hostname == "api.openai.com" and not key:
            raise ValueError("缺少语音 API 密钥。请在设置中填写语音 API 地址和密钥，或配置模型 API 密钥。")
        return url, key

    def transcribe(self, recording):
        root = self.offline_speech()
        if root is None:
            return asyncio.run(self.transcribe_async(recording))
        use_ncnn = all((root / name).is_file() for name in ("sherpa-ncnn", *NCNN_MODEL_FILES))
        if use_ncnn:
            command = [
                str(root / "sherpa-ncnn"), *(str(root / name) for name in NCNN_MODEL_FILES),
                str(recording), "2",
            ]
        else:
            command = [
                str(root / "sherpa-onnx"), "--num-threads=1", "--enable-endpoint=false",
                f"--zipformer2-ctc-model={root / 'model.int8.onnx'}",
                f"--tokens={root / 'tokens.txt'}", str(recording),
            ]
        try:
            result = subprocess.run(
                command,
                env=os.environ | {"LD_LIBRARY_PATH": str(root / "lib")},
                capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=90,
            )
        except subprocess.TimeoutExpired as error:
            raise ValueError("离线转写超过 90 秒，请尝试更短的录音。") from error
        except OSError as error:
            raise ValueError("无法启动离线语音引擎，请检查安装包。") from error
        if result.returncode:
            raise ValueError("离线语音引擎运行失败，请检查模型和可用内存。")
        if use_ncnn:
            for line in reversed(result.stdout.splitlines()):
                if line.startswith("text:"):
                    return NAS_STORAGE_NAME.sub(r"\1 NAS", line.removeprefix("text:").strip())
        else:
            for line in reversed(result.stdout.splitlines() + result.stderr.splitlines()):
                if line.lstrip().startswith("{"):
                    try:
                        payload = json.loads(line)
                    except ValueError:
                        continue
                    if isinstance(payload.get("text"), str):
                        return payload["text"].strip()
        raise ValueError("离线语音引擎没有返回识别结果。")

    async def transcribe_async(self, recording):
        import httpx

        url, key = self.speech_endpoint()
        headers = {"Authorization": f"Bearer {key}"} if key else {}
        try:
            async with asyncio.timeout(20):
                async with httpx.AsyncClient(timeout=httpx.Timeout(15, connect=5)) as client:
                    with recording.open("rb") as audio:
                        response = await client.post(
                            url.rstrip("/") + "/audio/transcriptions", headers=headers,
                            data={"model": self.config.values["speech.model"]},
                            files={"file": ("speech.wav", audio, "audio/wav")},
                        )
        except (TimeoutError, httpx.TimeoutException) as error:
            raise ValueError("转写连接或响应超时。请检查设备网络和语音 API 地址。") from error
        except httpx.RequestError as error:
            raise ValueError("连接转写服务失败。请检查设备网络和语音 API 地址。") from error
        if response.status_code in {401, 403}:
            raise ValueError("转写服务拒绝访问。请检查语音 API 密钥和模型权限。")
        if response.status_code in {404, 405}:
            raise ValueError("转写接口不可用。请检查语音 API 地址和转写模型。")
        if response.is_error or response.is_redirect:
            raise ValueError(f"转写服务返回 HTTP {response.status_code}。")
        try:
            text = response.json()["text"]
            if not isinstance(text, str):
                raise ValueError
        except (ValueError, KeyError, TypeError) as error:
            raise ValueError("转写接口返回格式有误。请确认地址支持 OpenAI 兼容转写。") from error
        return text.strip()

    def mcp(self, server_name, operation, name=None, arguments=None):
        server = next((s for s in self.config.values["mcp.servers"] if s["name"] == server_name), None)
        if not server:
            raise ValueError("未找到 MCP 服务器")
        allowed = {"list_tools", "call_tool", "list_resources", "list_prompts", "read_resource", "get_prompt"}
        if operation not in allowed:
            raise ValueError("MCP 操作无效")
        result = asyncio.run(mcp_request(server, operation, name, arguments))
        return json.dumps(result, ensure_ascii=False, default=lambda item: item.model_dump(mode="json"))


def send(kind, **fields):
    print(json.dumps({"type": kind, **fields}, ensure_ascii=False), flush=True)


def run_pipe(app):
    for line in sys.stdin:
        try:
            request = json.loads(line)
            command = request["type"]
            if command == "model_status":
                send("model_status", text=app.model_hint())
            elif command == "settings":
                send("settings", fields=app.config.public(), model_hint=app.model_hint())
            elif command == "save":
                app.save(request["key"], request["value"])
                send("saved")
            elif command == "catalog":
                send("catalog_items", items=app.catalog(request["kind"]))
            elif command == "memory_topics":
                send("memory_topics", files=app.memory_topics())
            elif command == "memory_read":
                send("memory_file", **app.read_memory(request["file"]))
            elif command == "memory_save":
                send("memory_saved", **app.save_memory(request["file"], request["text"], request["revision"]))
            elif command == "save_model":
                app.config.save_model(request["base_url"], request["model"], request.get("api_key"))
                app.reload_session()
                send("model_saved", fields=app.config.public())
            elif command == "submit":
                send("reply", text=app.ask(request["text"], lambda text: send("status", text=text)))
            elif command == "pinyin":
                send("pinyin", spelling=request["spelling"], text=pinyin_candidates(request["spelling"]))
            elif command == "voice_start":
                send("recording", backend=app.start_voice())
            elif command == "voice_stop":
                text = app.finish_voice()
                if not text:
                    raise ValueError("未识别到语音，请靠近麦克风重试。")
                send("draft", text=text)
            elif command == "voice_cancel":
                app.finish_voice(cancel=True)
                send("recording_cancelled")
            elif command == "new":
                app.close()
                app.setup(restore=False)
                send("reply", text="新会话")
            elif command == "quit":
                break
        except Exception as error:
            send("error", text=error_message(error))


def error_message(error):
    if isinstance(error, ConfigError):
        if str(error).startswith("Missing API key"):
            return "未配置模型 API Key。请打开 Esc → 设置 → 模型，填写连接后保存。"
        return "模型配置无效。请到 Esc → 设置 → 模型检查连接配置。"
    if isinstance(error, ValueError) and not isinstance(error, json.JSONDecodeError):
        return str(error)
    if isinstance(error, FileNotFoundError) and error.filename in {"arecord", "timeout", "parecord"}:
        return "找不到录音工具，请检查系统的 ALSA 或 PipeWire 音频工具"
    return f"操作失败 ({type(error).__name__})"


def repl(app):
    print("Colibri REPL  /new  /voice  /set KEY VALUE  /mcp SERVER OP [NAME] [JSON]  /quit")
    while True:
        try:
            text = input("› ").strip()
            if text == "/quit":
                return
            if text == "/new":
                app.close()
                app.setup(restore=False)
            elif text == "/voice":
                app.start_voice()
                choice = input("录音中，最多 60 秒。Enter 结束，输入 /cancel 取消 › ")
                draft = app.finish_voice(cancel=choice == "/cancel")
                if not draft:
                    continue
                if sys.stdin.isatty():
                    import readline

                    readline.set_startup_hook(lambda: readline.insert_text(draft))
                    try:
                        edited = input("语音草稿 › ").strip()
                    finally:
                        readline.set_startup_hook()
                    if edited:
                        print(app.ask(edited, print))
                else:
                    print(draft)
            elif text.startswith("/set "):
                key, value = text[5:].split(" ", 1)
                app.save(key, value)
            elif text.startswith("/mcp "):
                server, operation, *params = text[5:].split(maxsplit=3)
                name = params[0] if params else None
                arguments = json.loads(params[1]) if len(params) > 1 else None
                print(app.mcp(server, operation, name, arguments))
            elif text:
                print(app.ask(text, print))
        except (EOFError, KeyboardInterrupt):
            return
        except Exception as error:
            print(error_message(error))


def interrupt_worker(_signum, _frame):
    raise KeyboardInterrupt


def migrate_device_home(legacy, home):
    marker = home / ".cardputer-layout-v1"
    if not legacy.is_dir() or legacy.resolve() == home.resolve() or marker.exists():
        return
    files = [legacy / "settings.json", *(legacy / "transcripts").glob("*.jsonl")]
    for source in files:
        if source.is_symlink() or not source.is_file():
            continue
        destination = home / source.relative_to(legacy)
        destination.parent.mkdir(parents=True, exist_ok=True)
        if destination.exists():
            continue
        fd, temp = tempfile.mkstemp(dir=destination.parent)
        try:
            with os.fdopen(fd, "wb") as output, source.open("rb") as input_stream:
                shutil.copyfileobj(input_stream, output)
            try:
                os.link(temp, destination)
            except FileExistsError:
                pass
        finally:
            Path(temp).unlink(missing_ok=True)
    home.mkdir(parents=True, exist_ok=True)
    marker.touch(mode=0o600, exist_ok=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repl", action="store_true")
    parser.add_argument("--home", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    signal.signal(signal.SIGTERM, interrupt_worker)
    explicit_home = os.getenv("COLIBRI_CARDPUTER_HOME")
    home = args.home or (Path(explicit_home).expanduser() if explicit_home else colibri_home())
    if args.home is None and not explicit_home:
        migrate_device_home(Path.home() / ".local/share/colibri-cardputer", home)
    app = App(home)
    try:
        repl(app) if args.repl else run_pipe(app)
    except KeyboardInterrupt:
        pass
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        app.close()


if __name__ == "__main__":
    main()
