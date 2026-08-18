"""Arranque del sidecar: python3 -m nexum_hormiguero_sidecar

Cumple CHANGE-RUNTIME-001 (Sidecar Lifecycle Contract): primero el lock de
instancia, después token/bind/metadata. Escribe port/token/pid (0600) en el
runtime dir y sirve hasta SIGTERM. No imprime el token. No imprime secrets.
Logs mínimos a stderr.
"""

from __future__ import annotations

import os
import signal
import sys

from . import watchdog
from .auth import cleanup_files, generate_token, runtime_dir, write_pid, write_port
from .lifecycle import (
    EXIT_ALREADY_RUNNING,
    EXIT_UNHEALTHY_INSTANCE,
    acquire_instance_lock,
    clean_stale_metadata,
    probe_existing_instance,
)
from .server import build_server


def main() -> int:
    debug = os.environ.get("NEXUM_HORMIGUERO_DEBUG") == "1"

    # CHANGE-RUNTIME-001: el lock va ANTES de token/bind. Si otra instancia
    # lo sostiene, jamás tocar su metadata (token/port vigentes siguen
    # siendo válidos para los clientes).
    lock = acquire_instance_lock()
    if lock is None:
        if probe_existing_instance():
            print(
                "[hormiguero-sidecar] instancia sana ya corriendo — reuso",
                file=sys.stderr,
            )
            return EXIT_ALREADY_RUNNING
        print(
            "[hormiguero-sidecar] lock tomado por una instancia que no "
            "responde /health — no arranco (el launcher puede terminar el "
            "PID del pidfile validando cmdline y reintentar)",
            file=sys.stderr,
        )
        return EXIT_UNHEALTHY_INSTANCE

    # Lock en mano: pid/port/token preexistentes son de un dueño muerto.
    clean_stale_metadata()

    token = generate_token()
    activity = watchdog.Activity()
    server = build_server(
        token=token, debug=debug, allow_shutdown=True, activity=activity
    )
    port = server.server_address[1]
    write_port(port)
    write_pid()
    # Watchdog de ciclo de vida (OMEGA Fase 5, B-1): parent-PID declarado por
    # el lanzador y/o TTL de inactividad. El sidecar jamás sobrevive a su mundo.
    watchdog.start(
        server,
        activity,
        parent_pid=watchdog.parent_pid_from_env(),
        idle_ttl_secs=watchdog.idle_ttl_from_env(),
    )
    # Nota: se loggea el puerto (no sensible en loopback+token), nunca el token.
    print(
        f"[hormiguero-sidecar] escuchando en 127.0.0.1:{port} "
        f"(runtime dir: {runtime_dir()})",
        file=sys.stderr,
    )

    def _stop(_sig: int, _frm: object) -> None:
        # OJO: shutdown() directo desde el signal handler deadlockea —
        # espera a que serve_forever() avance, pero el handler corre EN el
        # main thread interrumpiéndolo (bug encontrado en el E2E Sprint 0).
        # Siempre desde otro thread.
        import threading

        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, _stop)
    signal.signal(signal.SIGINT, _stop)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        cleanup_files()
        # El flock muere con el proceso; cerrar explícito por prolijidad.
        lock.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
