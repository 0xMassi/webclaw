"""Run the webclaw command line tool from Python.

The package installs the `webclaw` binary next to this module and every function
here runs it once and parses what it prints. Nothing goes over the network
except the page fetches themselves.
"""

from __future__ import annotations

import functools
import importlib.metadata
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import warnings
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal, TypedDict, overload

if TYPE_CHECKING:
    from typing_extensions import Unpack

__all__ = [
    "Options",
    "WebclawError",
    "WebclawWarning",
    "binary_path",
    "crawl",
    "find_text",
    "scrape",
    "screenshot",
    "select",
    "similar",
    "tables",
]


class Options(TypedDict, total=False):
    """Fetch options every function accepts as keyword arguments."""

    proxy: str
    browser: Literal["chrome", "chrome-macos", "firefox", "safari", "edge", "safari-ios", "random"]
    headers: Mapping[str, str]
    cookies: str | Mapping[str, str]
    timeout: int
    render: Literal["auto", "always", "never"]
    cache: bool


class WebclawError(RuntimeError):
    """The binary exited with an error. `stderr` and `returncode` hold what it reported."""

    def __init__(self, message: str, stderr: str, returncode: int) -> None:
        super().__init__(message)
        self.stderr = stderr
        self.returncode = returncode


class WebclawWarning(UserWarning):
    """A warning the binary printed on a run that succeeded, such as a page with no content."""


_ANSI = re.compile(r"\x1b\[[0-9;]*m")


def _warn(stderr: str, stacklevel: int) -> None:
    for line in stderr.splitlines():
        line = _ANSI.sub("", line).strip()
        if "warning:" in line:
            warnings.warn(line.split("warning:", 1)[1].strip(), WebclawWarning, stacklevel=stacklevel)
        elif re.search(r"\bWARN\b", line):
            warnings.warn(line, WebclawWarning, stacklevel=stacklevel)


@functools.lru_cache(maxsize=None)
def binary_path() -> str:
    """Path of the `webclaw` binary installed with this package."""
    exe = "webclaw.exe" if os.name == "nt" else "webclaw"
    try:
        files = importlib.metadata.files("webclaw-cli") or []
    except importlib.metadata.PackageNotFoundError:
        files = []
    for file in files:
        if file.name == exe and Path(file.locate()).is_file():
            return str(Path(file.locate()).resolve())
    target = Path(__file__).resolve().parents[1] / "bin" / exe
    if target.is_file():
        return str(target)
    raise FileNotFoundError(f"cannot find the {exe} binary of the webclaw-cli package; reinstall it")


def _options(o: Options) -> list[str]:
    unknown = set(o) - Options.__optional_keys__
    if unknown:
        raise TypeError(f"unknown option {', '.join(sorted(unknown))}")
    args = []
    if "proxy" in o:
        args.append(f"--proxy={o['proxy']}")
    if "browser" in o:
        args.append(f"--browser={o['browser']}")
    for name, value in o.get("headers", {}).items():
        args.append(f"--header={name}: {value}")
    cookies = o.get("cookies")
    if cookies:
        if not isinstance(cookies, str):
            cookies = "; ".join(f"{k}={v}" for k, v in cookies.items())
        args.append(f"--cookie={cookies}")
    if "timeout" in o:
        args.append(f"--timeout={o['timeout']}")
    if "render" in o:
        args.append(f"--render={o['render']}")
    if o.get("cache"):
        args.append("--cache")
    return args


def _target(target: str) -> tuple[list[str], str | None]:
    if target.lstrip().startswith("<"):
        return ["--stdin"], target
    return ["--", target], None


def _error(stderr: str, returncode: int) -> WebclawError:
    lines = [line for line in stderr.splitlines() if line.strip()]
    errors = [line for line in lines if line.startswith("error: ")]
    message = (errors or lines or [f"webclaw exited with code {returncode}"])[-1].removeprefix("error: ")
    return WebclawError(message, stderr, returncode)


def _spawn(cmd: list[str], stdin: bool, stderr: Any) -> subprocess.Popen[str]:
    return subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=stderr,
        text=True,
        encoding="utf-8",
        env={**os.environ, "NO_COLOR": "1"},
        start_new_session=os.name != "nt",
    )


def _kill(proc: subprocess.Popen[str]) -> None:
    if os.name == "nt":
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)], capture_output=True)
    else:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
    proc.wait()
    for profile in Path(tempfile.gettempdir()).glob(f"webclaw-chrome-{proc.pid}-*"):
        shutil.rmtree(profile, ignore_errors=True)


def _run(args: list[str], target: str, options: Options) -> str:
    source, stdin = _target(target)
    with _spawn([binary_path(), *args, *_options(options), *source], stdin is not None, subprocess.PIPE) as proc:
        try:
            out, err = proc.communicate(stdin)
        except BaseException:
            _kill(proc)
            raise
    if proc.returncode != 0:
        raise _error(err, proc.returncode)
    _warn(err, 4)
    return out


def _comma_list(flag: str, values: str | Sequence[str]) -> list[str]:
    items = [values] if isinstance(values, str) else list(values)
    for item in items:
        if "," in item:
            raise ValueError(f"{flag} items cannot contain a comma, the binary splits on it: {item!r}")
    return [f"{flag}={','.join(items)}"] if items else []


Format = Literal["markdown", "text", "llm", "html"]


@overload
def scrape(
    target: str,
    format: Format = ...,
    *,
    query: str | None = ...,
    include: Sequence[str] = ...,
    exclude: Sequence[str] = ...,
    only_main_content: bool = ...,
    **options: Unpack[Options],
) -> str: ...
@overload
def scrape(
    target: str,
    format: Literal["json"],
    *,
    query: str | None = ...,
    include: Sequence[str] = ...,
    exclude: Sequence[str] = ...,
    only_main_content: bool = ...,
    **options: Unpack[Options],
) -> dict[str, Any]: ...
def scrape(
    target: str,
    format: Format | Literal["json"] = "markdown",
    *,
    query: str | None = None,
    include: Sequence[str] = (),
    exclude: Sequence[str] = (),
    only_main_content: bool = False,
    **options: Unpack[Options],
) -> str | dict[str, Any]:
    """Extract a page as markdown, text, llm, html, or json (a dict).

    `target` is a URL, or an HTML string when it starts with `<`. `query` keeps
    only the parts of the page that answer the question.
    """
    args = [f"--format={format}"]
    if query:
        args.append(f"--query={query}")
    args += _comma_list("--include", include) + _comma_list("--exclude", exclude)
    if only_main_content:
        args.append("--only-main-content")
    out = _run(args, target, options)
    return json.loads(out) if format == "json" else out


def _fields(fields: Mapping[str, str] | None) -> list[str]:
    return [f"--field={name}={selector}" for name, selector in (fields or {}).items()]


def select(
    target: str,
    css: str | None = None,
    xpath: str | None = None,
    fields: Mapping[str, str] | None = None,
    **options: Unpack[Options],
) -> list[Any]:
    """What a CSS selector or an XPath expression picks.

    A CSS selector can end in `::text` (the default), `::attr(NAME)` or `::html`.
    With `fields` ({name: selector run inside each match}) each match becomes a dict.
    """
    if (css is None) == (xpath is None):
        raise ValueError("pass exactly one of css or xpath")
    pick = f"--select={css}" if css is not None else f"--xpath={xpath}"
    return json.loads(_run([pick, *_fields(fields)], target, options))


def tables(target: str, **options: Unpack[Options]) -> list[dict[str, Any]]:
    """The page's data tables: caption, headers and rows of cell text."""
    return json.loads(_run(["--tables"], target, options))


def find_text(
    target: str,
    text: str,
    *,
    regex: bool = False,
    exact: bool = False,
    case_sensitive: bool = False,
    **options: Unpack[Options],
) -> list[dict[str, str]]:
    """The deepest elements whose text contains `text`, each with a CSS selector."""
    args = [f"--find-regex={text}" if regex else f"--find-text={text}"]
    if exact:
        args.append("--exact")
    if case_sensitive:
        args.append("--case-sensitive")
    return json.loads(_run(args, target, options))


def similar(
    target: str,
    *,
    css: str | None = None,
    xpath: str | None = None,
    text: str | None = None,
    fields: Mapping[str, str] | None = None,
    **options: Unpack[Options],
) -> dict[str, Any]:
    """Every element like the first one the example picks, and one CSS selector for all of them.

    Returns {"selector", "matches"}, or {"selector", "records"} with `fields`.
    """
    examples = [f"--select={css}"] if css is not None else []
    examples += [f"--xpath={xpath}"] if xpath is not None else []
    examples += [f"--find-text={text}"] if text is not None else []
    if len(examples) != 1:
        raise ValueError("pass exactly one of css, xpath or text")
    return json.loads(_run([*examples, "--similar", *_fields(fields)], target, options))


def crawl(
    url: str,
    *,
    max_pages: int = 20,
    depth: int = 1,
    query: str | None = None,
    **options: Unpack[Options],
) -> Iterator[dict[str, Any]]:
    """Crawl same-site links and yield each page as it finishes.

    Each page is a dict with `url`, `depth`, `extraction` (the `scrape(..., "json")`
    shape) and `error`, set when that page failed. It raises `WebclawError` only when
    no page succeeded. `max_pages=0` means no limit. Stopping the loop early stops
    the crawl, and the browser it started.
    """
    args = ["--crawl", "--format=jsonl", f"--max-pages={max_pages}", f"--depth={depth}"]
    if query:
        args.append(f"--query={query}")
    cmd = [binary_path(), *args, *_options(options), "--", url]
    with tempfile.TemporaryFile() as err, _spawn(cmd, False, err) as proc:
        assert proc.stdout is not None
        pages = failed = 0
        try:
            for line in proc.stdout:
                if line.strip():
                    page = json.loads(line)
                    pages += 1
                    failed += bool(page.get("error"))
                    yield page
        except BaseException:
            _kill(proc)
            raise
        proc.wait()
        err.seek(0)
        stderr = err.read().decode("utf-8", "replace")
        if proc.returncode != 0 and failed == pages:
            raise _error(stderr, proc.returncode)
        _warn(stderr, 3)


def map(url: str, *, limit: int | None = None, **options: Unpack[Options]) -> list[str]:
    """URLs a site lists in its sitemaps, with a short crawl when they list few."""
    args = ["--map"] + ([f"--map-limit={limit}"] if limit is not None else [])
    return _run(args, url, options).split()


def screenshot(
    url: str, path: str | os.PathLike[str], *, full_page: bool = False, **options: Unpack[Options]
) -> Path:
    """Save a 1280x720 screenshot (the whole page with `full_page`) as PNG, or JPEG for a .jpg path.

    Needs Chrome or Chromium installed.
    """
    path = Path(path)
    _run([f"--screenshot={path}", *(["--full-page"] if full_page else [])], url, options)
    return path
