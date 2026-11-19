"""The engine's test chunks run their executables directly (not through cargo), so they must get
the environment cargo would give them: the `[env]` table of .cargo/config.toml. Without it a
test thread has Rust's 2 MB default stack instead of RUST_MIN_STACK, and a deep layout test
overflows it in a Windows debug build, ending the whole chunk."""
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import collect_metrics  # noqa: E402

REPO = Path(__file__).resolve().parent.parent.parent


def test_chunk_processes_get_cargos_env_table():
    config = tomllib.loads((REPO / ".cargo" / "config.toml").read_text(encoding="utf-8"))
    wanted = config["env"]["RUST_MIN_STACK"]
    env = collect_metrics.chunk_env({})
    assert env["RUST_MIN_STACK"] == wanted


def test_an_env_var_already_set_wins_as_it_does_under_cargo():
    env = collect_metrics.chunk_env({"RUST_MIN_STACK": "123"})
    assert env["RUST_MIN_STACK"] == "123"


def test_every_engine_process_is_started_with_that_env():
    source = Path(collect_metrics.__file__).read_text(encoding="utf-8")
    body = source[source.index("def run_tests("):]
    body = body[: body.index("\ndef ", 1)]
    calls = body.split("subprocess.run(")[1:]
    assert calls, "run_tests starts its processes with subprocess.run"
    for call in calls:
        assert "env=" in call.split("\n\n")[0], call[:200]


def test_failures_and_a_dead_process_are_named(capsys):
    body = (
        "running 3 tests\n"
        "test a::passes ... ok\n"
        "test a::fails ... FAILED\n"
        "test b::dies ... "
    )
    collect_metrics.name_failures("lib", body)
    err = capsys.readouterr().err
    assert "lib: FAILED a::fails" in err
    assert "lib: the process ended during b::dies" in err
    assert "a::passes" not in err


def test_a_run_that_finished_names_no_dead_test(capsys):
    collect_metrics.name_failures("lib", "test a::x ... ok\ntest result: ok. 1 passed\n")
    assert capsys.readouterr().err == ""


def test_a_failure_is_printed_with_its_panic_message(capsys):
    body = (
        "test a::fails ... FAILED\n\nfailures:\n\n---- a::fails stdout ----\n\n"
        "thread 'a::fails' panicked at src/lib.rs:3:5:\nexpected 400, got 270\n\nfailures:\n    a::fails\n"
    )
    collect_metrics.name_failures("workspace", body)
    err = capsys.readouterr().err
    assert "workspace: FAILED a::fails" in err
    assert "expected 400, got 270" in err


def test_the_adapter_probe_is_read_from_its_line(monkeypatch):
    class Done:
        def __init__(self, out):
            self.stdout = out
    outputs = iter(["running 1 test\nTEST ADAPTER: Cpu\ntest test_adapter_report ... ok\n",
                    "TEST ADAPTER: DiscreteGpu\n", "no line here\n"])
    monkeypatch.setattr(collect_metrics.subprocess, "run", lambda *a, **k: Done(next(outputs)))
    assert collect_metrics.test_adapter("exe", {}) == "Cpu"
    assert collect_metrics.test_adapter("exe", {}) == "DiscreteGpu"
    assert collect_metrics.test_adapter("exe", {}) == "unknown"


def test_skipped_tests_count_against_the_listed_total():
    source = Path(collect_metrics.__file__).read_text(encoding="utf-8")
    assert 'info["ran"] + info["skipped"] != info["listed"]' in source
    assert "full suite runs on a hardware GPU" in collect_metrics.SOFTWARE_ADAPTER_SKIP
