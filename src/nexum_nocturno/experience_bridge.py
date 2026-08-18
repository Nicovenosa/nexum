"""Experience Bridge (OMEGA Live Wiring Fase B).

Conecta el Nocturno a la evidencia REAL que el producto (runtime Rust) escribe
en ~/.nexum/experience/evidence.jsonl. Valida la hash chain con el MISMO esquema
que el writer Rust (planning/evidence.rs) y rechaza evidencia corrupta. Mapea los
eventos de ciclo de vida a un replay dataset que `NocturnoEngine.detect_patterns`
consume. No usa fixtures ni tempdirs de tests: solo evidencia productiva.

Esquema de cadena (idéntico a Rust):
    material = f"{prev}|{ts}|{trace_id}|{task_id}|{plan_id}|{lifecycle}|"
               f"{component}|{provenance}|{input_hash}|{output_hash}"
    entry_hash = sha256(material)  # hex
    prev inicial = "genesis"
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any


class CorruptEvidenceError(ValueError):
    """La cadena de evidencia real falló la verificación: NO se consume."""


def default_evidence_path() -> Path:
    import os

    override = os.environ.get("NEXUM_EXPERIENCE_DIR")
    base = Path(override) if override else Path.home() / ".nexum" / "experience"
    return base / "evidence.jsonl"


def _entry_material(prev: str, r: dict[str, Any]) -> str:
    return (
        f"{prev}|{r.get('ts_ms', 0)}|{r.get('trace_id', '')}|{r.get('task_id', '')}|"
        f"{r.get('plan_id') or ''}|{r.get('lifecycle', '')}|{r.get('component', '')}|"
        f"{r.get('provenance', '')}|{r.get('input_hash', '')}|{r.get('output_hash', '')}"
    )


def load_and_verify(path: Path | None = None) -> list[dict[str, Any]]:
    """Lee la evidencia real y VERIFICA la hash chain. Cualquier ruptura ⇒
    CorruptEvidenceError (fail-closed: evidencia corrupta jamás se consume)."""
    path = path or default_evidence_path()
    if not path.exists():
        return []
    records: list[dict[str, Any]] = []
    prev = "genesis"
    for i, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
        except json.JSONDecodeError as e:
            raise CorruptEvidenceError(f"línea {i} no es JSON: {e}") from e
        expect = hashlib.sha256(_entry_material(prev, r).encode()).hexdigest()
        if r.get("prev_hash") != prev or r.get("entry_hash") != expect:
            raise CorruptEvidenceError(
                f"cadena rota en registro {i}: prev/entry no coinciden"
            )
        prev = r["entry_hash"]
        records.append(r)
    return records


def to_replay_dataset(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Mapea eventos de ciclo de vida REALES al replay dataset que
    `detect_patterns` consume. Sin datos privados: solo estructura + resultado."""
    replay: list[dict[str, Any]] = []
    for r in records:
        lifecycle = r.get("lifecycle", "")
        # outcome derivado del tipo de evento (worker_error ⇒ failure)
        outcome = "failure" if lifecycle == "worker_error" else "success"
        replay.append(
            {
                "task_class": "planning" if r.get("plan_id") else "generic",
                "route_selected": lifecycle,
                "latency_ms_total": 0,
                "outcome": outcome,
                "dedup_count": 1,
                "trace_id": r.get("trace_id", ""),
                "provenance": r.get("provenance", ""),
            }
        )
    return replay


def contains_private_data(records: list[dict[str, Any]]) -> list[str]:
    """Verifica que NINGÚN registro real contenga texto crudo/privado. Solo se
    admiten hashes (64 hex) o campos estructurales conocidos. Devuelve la lista
    de violaciones (vacía = privacidad por construcción OK)."""
    allowed_keys = {
        "schema_version", "ts_ms", "trace_id", "task_id", "plan_id", "lifecycle",
        "component", "provenance", "input_hash", "output_hash", "prev_hash", "entry_hash",
    }
    violations: list[str] = []
    for i, r in enumerate(records):
        extra = set(r.keys()) - allowed_keys
        if extra:
            violations.append(f"registro {i}: claves inesperadas {sorted(extra)}")
        # input/output_hash deben ser hash (64 hex) o vacío o código corto conocido.
        for f in ("input_hash", "output_hash"):
            v = str(r.get(f, ""))
            if v and not (len(v) == 64 and all(c in "0123456789abcdef" for c in v)):
                # se permiten códigos cortos de error/capability (no privados)
                if len(v) > 40:
                    violations.append(f"registro {i}: {f} sospechoso de dato crudo")
    return violations
