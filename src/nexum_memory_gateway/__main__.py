"""Arranque del sidecar: python3 -m nexum_memory_gateway

Cumple CHANGE-RUNTIME-001: flag → lock de instancia → token/bind/metadata.
Flag NEXUM_MEMORY OFF por defecto (D-13): exit 3 sin side-effects.
No imprime el token. Logs mínimos a stderr sin contenido sensible.
"""

from __future__ import annotations

import os
import signal
import sys

from .auth import (
    cleanup_files,
    db_path,
    generate_token,
    runtime_dir,
    write_pid,
    write_port,
)
from .lifecycle import (
    EXIT_ALREADY_RUNNING,
    EXIT_FLAG_OFF,
    EXIT_UNHEALTHY_INSTANCE,
    acquire_instance_lock,
    clean_stale_metadata,
    probe_existing_instance,
)
from . import watchdog
from .server import build_server
from .store import MemoryStore


def main() -> int:
    flag = os.environ.get("NEXUM_MEMORY", "").lower()
    if flag not in ("1", "true", "on", "yes"):
        print(
            "[memory-gateway] NEXUM_MEMORY off: cero lecturas/escrituras, "
            "backend no requerido",
            file=sys.stderr,
        )
        return EXIT_FLAG_OFF

    lock = acquire_instance_lock()
    if lock is None:
        if probe_existing_instance():
            print(
                "[memory-gateway] instancia sana ya corriendo — reuso",
                file=sys.stderr,
            )
            return EXIT_ALREADY_RUNNING
        print(
            "[memory-gateway] lock tomado por una instancia que no responde "
            "/health — no arranco (el launcher puede terminar el PID del "
            "pidfile validando cmdline y reintentar)",
            file=sys.stderr,
        )
        return EXIT_UNHEALTHY_INSTANCE

    clean_stale_metadata()
    store = MemoryStore(db_path())
    token = generate_token()
    activity = watchdog.Activity()
    server = build_server(store=store, token=token, activity=activity)
    server.RequestHandlerClass.counters.bump("sidecar_starts")
    port = server.server_address[1]
    write_port(port)
    write_pid()
    # Watchdog de ciclo de vida (OMEGA Fase 5, B-1): el gateway jamás
    # sobrevive a su mundo (parent-PID declarado y/o TTL de inactividad).
    watchdog.start(
        server,
        activity,
        parent_pid=watchdog.parent_pid_from_env(),
        idle_ttl_secs=watchdog.idle_ttl_from_env(),
        service_name="memory-gateway",
    )
    # Se loggea el puerto (no sensible en loopback+token) y el estado de la
    # DB; jamás el token ni contenido.
    print(
        f"[memory-gateway] escuchando en 127.0.0.1:{port} "
        f"(runtime dir: {runtime_dir()}, db_state: {store.db_state}, "
        f"search: {store.search_backend if store.db_state == 'ok' else 'n/a'})",
        file=sys.stderr,
    )

    def _stop(_sig: int, _frm: object) -> None:
        # shutdown() SIEMPRE desde otro thread (deadlock conocido del
        # patrón sidecar si se llama desde el signal handler).
        import threading

        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, _stop)
    signal.signal(signal.SIGINT, _stop)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        cleanup_files()
        store.close()
        lock.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
