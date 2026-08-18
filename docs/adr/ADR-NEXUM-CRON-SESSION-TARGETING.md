# ADR: Cron apunta a una sesion existente

- **Estado:** Aceptado
- **Fecha:** 2026-07-10
- **Alcance:** `nexum-acp/src/cron` y `nexum-acp-host`

## Contexto

Las ejecuciones programadas necesitan contexto conversacional, herramientas y persistencia que ya existen en ACP. Crear una sesion o un loop de agente paralelo para cada vencimiento separaria el historial, duplicaria configuracion de provider/middlewares y dificultaria preservar las reglas de permisos e HITL.

Tambien hay dos clases de identidad distintas. Una conexion ACP tiene un ID efimero util para routing, mientras que una interaccion durable necesita un owner que sobreviva reconexiones. El host local ademas debe decidir de donde sale esa identidad sin aceptar datos controlados por el cliente.

## Decision

Un `CronJob` persiste un `target_thread_id` y ejecuta el `prompt` programado contra ese thread existente. El runtime no crea una conversacion, ThreadStore, provider ni executor alternativo.

La implementacion concreta sigue esta cadena:

```text
CronJob.target_thread_id
  -> HeadlessPromptContextFactory carga metadata + historial
  -> SessionManager.ensure_session / build_frozen_data
  -> PromptExecutionContext(session_id = target_thread_id)
  -> session::executor::execute_prompt
  -> append_messages del delta al mismo ThreadStore
```

Cada `target_thread_id` tiene una SessionLane local (`Mutex`) para serializar corridas del mismo thread. Las lanes preservan el orden de persistencia del historial dentro de un proceso; no son un lock distribuido.

El owner durable se captura en `CronJobSpec.owner_principal`, se guarda en `cron_jobs.owner_principal` y se copia a `cron_pending_interactions.owner_principal`. `CallerContext.connection_id` queda deliberadamente fuera de SQLite. Solo el host autentica y adjunta el `HostPrincipal` al request.

En el host local Linux, `SO_PEERCRED` suministra el UID del peer. Se acepta un stream solo si ese UID coincide con el effective UID del proceso host; el principal queda representado como `unix-uid:<uid>`. `OwnerPrincipalAuthorizer` exige igualdad exacta entre ese principal y el owner de la interaccion para leer o resolver.

## Consecuencias

### Positivas

- El cron usa el executor ACP existente y su configuracion normal, en vez de sostener un segundo camino de agente.
- El historial, `cwd`, datos frozen y herramientas se recuperan del thread objetivo.
- Las corridas del mismo thread no compiten por el historial dentro del mismo runtime.
- Una reconexion puede conservar el mismo principal durable aunque cambie el ID de conexion.
- Las interacciones headless quedan auditables sin conceder permisos ni inventar respuestas.

### Restricciones aceptadas

- Una corrida no puede seguir si llega a HITL. `HeadlessFailSafeBroker` persiste el contexto, rechaza/contesta vacio, cancela y el run queda `failed_needs_user`.
- Aprobar o rechazar el registro pendiente no continua el agente: `ContinuationCapability::Unsupported` y `continuationSupported: false` son contractuales.
- El principal Unix actual es un UID Linux, no una identidad de usuario Nexum. El host no acepta peers de otro UID.
- Registros legacy sin owner se deniegan; no hay fallback a ID de conexion, `actorId` ni otro parametro ACP.
- La gestion ACP disponible es solo de interacciones pendientes. No implica CRUD de jobs, pause/resume ni observabilidad en vivo.
- Una recuperacion de `running` sin interaccion durable la reencola; si hubo efectos antes de una caida, pueden repetirse. No se ofrece exactly-once ni deduplicacion de herramientas.

## Alternativas descartadas

| Alternativa | Motivo para no adoptarla |
|---|---|
| Crear una sesion por vencimiento | Perderia continuidad de historial y duplicaria la infraestructura de sesion. |
| Ejecutar un agente cron independiente | Duplicaria provider, middlewares, persistencia y semantica de cancelacion del executor ACP. |
| Persistir el ID de conexion como owner | Es efimero, se invalida con reconexiones y no es una identidad durable. |
| Tomar owner desde un parametro JSON-RPC | El cliente podria falsificarlo; `HostPrincipal` debe ser autenticado por el host. |
| Esperar HITL sin cliente | Bloquearia una corrida headless y podria llevar a aprobaciones o respuestas inventadas. |
| Reanudar despues de aprobar | No se conserva un agente pausado ni una capacidad de continuacion segura en la implementacion actual. |

## Validacion vigente

Las pruebas de `cron::mod_test` cubren persistencia/migracion, claim unico, recuperacion con y sin interaccion durable, SessionLane, resultado exitoso, estado `failed_needs_user`, FailSafely y autorizacion owner/reconexion. Las pruebas de servidor y hub comprueban que la resolucion exige caller y que el principal autenticado se propaga sin persistir el ID de conexion.

No hay una prueba E2E con provider HTTP real/mock que ejecute el binario host, cree un job por una API publica y complete un prompt end-to-end. Esa API de alta de jobs no existe en ACP actual.
