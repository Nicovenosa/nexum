"""CLI de Nocturno: python3 -m nexum_nocturno <subcmd> [args] → JSON a stdout.

Subcomandos: status | candidates | inspect <id> | approve <id> | reject <id>
<reason> | rollback <id> | history. Superficie para los comandos /nocturno
del runtime (OMEGA ciclo 6b). Vista pública segura: jamás contenido privado.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

from nexum_experience import EvidenceStore

from .candidates import CandidateStore, PromotionState
from .engine import Nocturno, autopromote_enabled, mode_from_env


def _base_dir() -> Path:
    root = os.environ.get("NEXUM_NOCTURNO_DIR", "")
    if root:
        return Path(root)
    return Path.home() / ".nexum" / "nocturno"


def _evidence_db() -> Path:
    return Path(
        os.environ.get(
            "NEXUM_EVIDENCE_DB",
            str(Path.home() / ".nexum" / "experience" / "evidence.db"),
        )
    )


def _cand_public(c) -> dict:
    return {
        "candidate_id": c.candidate_id,
        "kind": c.kind,
        "hypothesis_code": c.hypothesis_code,
        "state": c.state.value,
        "policy_version_base": c.policy_version_base,
        "baseline_metrics": c.baseline_metrics,
        "candidate_metrics": c.candidate_metrics,
        "reject_reason": c.reject_reason,
        "evidence_ids": c.evidence_ids,
    }


def _noop_evaluator(_policy: dict, _dataset: list) -> dict:
    # El CLI no benchmarkea: solo inspección/approve/reject/rollback.
    raise RuntimeError("el CLI no ejecuta benchmarks; usá el harness de Nocturno")


def main(argv: list[str]) -> int:
    cmd = argv[0] if argv else "status"
    base = _base_dir()
    if cmd == "status":
        store = CandidateStore(base / "candidates.db")
        counts: dict[str, int] = {}
        for c in store.list_by_state():
            counts[c.state.value] = counts.get(c.state.value, 0) + 1
        policy_path = base / "policy.json"
        policy_version = "-"
        if policy_path.exists():
            policy_version = str(json.loads(policy_path.read_text()).get("version"))
        ev = EvidenceStore(_evidence_db())
        ok, n = ev.verify_chain()
        print(
            json.dumps(
                {
                    "ok": True,
                    "mode": mode_from_env().value,
                    "autopromote": autopromote_enabled(),
                    "policy_version": policy_version,
                    "candidates_by_state": counts,
                    "evidence_records": n,
                    "evidence_chain_ok": ok,
                }
            )
        )
        ev.close()
        store.close()
        return 0
    if cmd in ("candidates", "history"):
        store = CandidateStore(base / "candidates.db")
        state = None if cmd == "history" else PromotionState.APPROVAL_PENDING
        cands = store.list_by_state(state)
        if cmd == "candidates" and not cands:
            cands = store.list_by_state()  # sin pendientes: mostrar todos
        print(json.dumps({"ok": True, "candidates": [_cand_public(c) for c in cands]}))
        store.close()
        return 0
    if cmd == "inspect" and len(argv) > 1:
        store = CandidateStore(base / "candidates.db")
        c = store.load(argv[1])
        store.close()
        if c is None:
            print(json.dumps({"ok": False, "error": "not_found"}))
            return 1
        print(json.dumps({"ok": True, "candidate": _cand_public(c)}))
        return 0
    if cmd in ("approve", "reject", "rollback") and len(argv) > 1:
        ev = EvidenceStore(_evidence_db())
        noct = Nocturno(base, ev, _noop_evaluator)
        try:
            if cmd == "approve":
                c = noct.approve(argv[1])
            elif cmd == "rollback":
                c = noct.rollback(argv[1])
            else:
                c = noct.reject(argv[1], argv[2] if len(argv) > 2 else "manual")
            print(json.dumps({"ok": True, "candidate": _cand_public(c)}))
            return 0
        except Exception as e:  # noqa: BLE001 (CLI: error → JSON, exit 1)
            print(json.dumps({"ok": False, "error": type(e).__name__}))
            return 1
        finally:
            ev.close()
    print(json.dumps({"ok": False, "error": "usage"}))
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
