"""Run the webclaw command line tool from Python.

The package installs the `webclaw` binary next to this module and every function
here runs it once and parses what it prints. Nothing goes over the network
except the page fetches themselves.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sysconfig
import tempfile
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal, TypedDict, overload

if TYPE_CHECKING:
    from typing_extensions import Unpack

__all__ = [
    "Options",
    "WebclawError",
    "binary_path",
    "crawl",
    "find_text",
    "map",
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


def binary_path() -> str:
    """Path of the `webclaw` binary this package runs."""
    exe = "webclaw" + (sysconfig.get_config_var("EXE") or "")
    dirs = [
        sysconfig.get_path("scripts"),
        sysconfig.get_path("scripts", sysconfig.get_preferred_scheme("user")),
        str(Path(__file__).resolve().parents[1] / "bin"),
    ]
    for d in dirs:
        path = os.path.join(d, exe)
        if os.path.isfile(path):
            return path
    found = shutil.which(exe)
    if found:
        return found
    raise FileNotFoundError(f"cannot find the {exe} binary; reinstall the webclaw-cli package")


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


def _run(args: list[str], target: str, options: Options) -> str:
    source, stdin = _target(target)
    r = subprocess.run(
        [binary_path(), *args, *_options(options), *source],
        input=stdin,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if r.returncode != 0:
        raise _error(r.stderr, r.returncode)
    return r.stdout


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
    if include:
        args.append(f"--include={','.join(include)}")
    if exclude:
        args.append(f"--exclude={','.join(exclude)}")
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
    shape) and `error`. `max_pages=0` means no limit. Stopping the loop early stops
    the crawl.
    """
    args = ["--crawl", "--format=jsonl", f"--max-pages={max_pages}", f"--depth={depth}"]
    if query:
        args.append(f"--query={query}")
    cmd = [binary_path(), *args, *_options(options), "--", url]
    with tempfile.TemporaryFile() as err, subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=err, text=True, encoding="utf-8"
    ) as proc:
        assert proc.stdout is not None
        try:
            for line in proc.stdout:
                if line.strip():
                    yield json.loads(line)
        except BaseException:
            proc.kill()
            raise
        if proc.wait() != 0:
            err.seek(0)
            raise _error(err.read().decode("utf-8", "replace"), proc.returncode)


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
