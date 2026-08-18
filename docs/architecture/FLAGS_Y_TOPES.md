# Flags de runtime y topes del loop — estado y cómo volver atrás

**Última actualización:** 2026-07-26
**Aplica a:** `nexum-acp` (Rust) y `nexum_providers` (Python)

Este documento es la referencia de **qué está encendido por defecto** y **cómo se
apaga**. Si un comportamiento cambió y no sabés por qué, empezá acá.

---

## 1. Los dos flags pasaron a default ON

Ambos nacieron como opt-in. Los dos dejaron de tener sentido como opt-in por el
mismo motivo: **el resguardo protegía contra encender algo nuevo, y terminó
protegiendo el comportamiento roto.** Un default que esconde lo que el sistema
ya sabe hacer no es un resguardo, es un bug con buena prensa.

| Variable | Antes | Ahora | Qué hace cuando está ON |
|---|---|---|---|
| `NEXUM_LOCAL_FAST` | opt-in, default OFF | **default ON** | Rutea el turno a `DIRECT_CHAT` cuando el texto no tiene señal de intención de tarea |
| `NEXUM_PROVIDER_CONNECT_V2` | opt-in, default OFF | **default ON** | El resolver prueba credenciales reales y promueve providers a `usable_now` |

### Regla de polaridad

> **Las variables quedan como interruptor para apagar, no para prender.**

- **Apagan:** `0`, `false`, `off`, `no` (case-insensitive, con trim)
- **No apagan:** cualquier otra cosa, incluida la variable **ausente** o vacía

Esto es deliberado: **falla hacia el comportamiento sano**. Un typo
(`NEXUM_LOCAL_FAST=of`) no devuelve el cuelgue ni deja al usuario sin modelos.
Los valores de encendido viejos (`1`, `true`, `on`, `yes`) siguen encendiendo,
así que quien los tenga exportados de antes no nota el cambio.

---

## 2. Cómo volver atrás

### 2.1 Sin recompilar ni redesplegar — apagar el flag

Es el camino de primera línea. No requiere tocar el slot.

```bash
# Vuelve al flujo completo con herramientas en todos los turnos
NEXUM_LOCAL_FAST=0 nexum

# Vuelve al catálogo sin resolver (un solo provider usable)
NEXUM_PROVIDER_CONNECT_V2=0 nexum-reconcile
```

Para dejarlo apagado de forma persistente, exportarlo en el perfil de la shell.
**El catálogo no se regenera solo:** después de apagar
`NEXUM_PROVIDER_CONNECT_V2` hay que correr `reconcile` para que el cambio se
refleje en `provider-catalog-live.json`.

### 2.2 Rollback del slot completo

Si el problema no es el flag sino el slot, el swap es atómico y reversible:

```bash
ln -sfn ~/.local/lib/nexum/0.1.4-rc.4-wizardfix-00cc150 /tmp/nexum-current.new
mv -Tf /tmp/nexum-current.new ~/.local/lib/nexum/current
```

`wizardfix-00cc150` es el slot inmediatamente anterior a este cambio: tiene el
fix de `needs_setup` pero conserva los flags en default OFF y el tope de 500.

Verificar después del swap:

```bash
nexum-verify-parity        # debe responder SÍ
nexum doctor               # 0 FAIL
```

---

## 3. Topes del loop de ReAct

| Camino | Tope | Constante |
|---|---|---|
| Turno interactivo (sin envelope) | **15** | `MAX_ITERATIONS_INTERACTIVE` |
| Turno con envelope explícito (`allowed_tools` presente) | **500** | `MAX_ITERATIONS_ENVELOPE` |

Antes era 500 para todo. A ~2,3 s por vuelta eso son hasta **19 minutos de
pantalla muda** antes de que el loop se corte solo: en la práctica, un cuelgue.
Una tarea interactiva que necesita más de 15 llamadas a herramientas es una que
el usuario debería estar guiando, no mirando.

El 500 se conserva sólo para el camino con envelope, que es donde una tarea
larga de agente tiene sentido y nadie está esperando frente a la pantalla.

Los topes se eligen en `tope_de_iteraciones()`
(`nexum-acp/src/agent/builder.rs`). No están detrás de una variable de entorno:
cambiarlos requiere recompilar, a propósito.

> **No usar `allowed_tools.is_some()` como proxy de "hay envelope".** DIRECT_CHAT
> también manda `Some(vec![])` —cero herramientas, no un sobre— así que con ese
> proxy un "hola" se llevaba el tope de 500. La señal es
> `AcpAgentConfig::has_explicit_envelope`, que viene de
> `task_envelope.is_some()`. Es el mismo patrón que `api_key.is_empty()`
> significando "no configurado": preguntarle a un campo algo que no responde.
> El test `direct_chat_no_cuenta_como_envelope` lo blinda.

### 3.1 Al agotarse el tope, el usuario se entera

> **Un tope sin mensaje es el mismo cuelgue, más corto.**

Antes, agotar el tope producía `Max iterations exceeded (500)` — en inglés, sin
decir qué pasó ni qué se llegó a hacer. Ahora `mensaje_tope_agotado()`
(`nexum-acp/src/session/executor.rs`) arma un mensaje que incluye:

- contra qué tope chocó,
- las herramientas que **sí** corrió, con su conteo,
- el último texto que alcanzó a producir (truncado a 600 caracteres),
- qué hacer a continuación.

El estado parcial no se tira: es lo que el turno sí consiguió.

---

## 4. Tests que fijan todo esto

Si alguien vuelve a invertir la polaridad o a unificar los topes, falla acá y no
en producción con un turno colgado.

| Qué fija | Dónde |
|---|---|
| Polaridad de `NEXUM_LOCAL_FAST` | `nexum-acp/src/flow_test.rs` — `sin_la_variable_el_ruteo_esta_encendido`, `la_variable_solo_sirve_para_apagar`, `un_valor_desconocido_no_apaga_el_ruteo` |
| Polaridad de `NEXUM_PROVIDER_CONNECT_V2` | `tests/test_provider_resolver_source.py` — `test_flag_ausente_activa_el_resolver`, `test_la_variable_solo_sirve_para_apagar`, `test_valor_vacio_o_desconocido_no_apaga` |
| Topes diferenciados | `nexum-acp/src/agent/builder.rs` — `tope_tests` |
| Que DIRECT_CHAT no cuente como envelope | `nexum-acp/src/agent/builder.rs` — `direct_chat_no_cuenta_como_envelope` |
| Mensaje de tope agotado | `nexum-acp/src/session/executor_test.rs` — `tope_agotado_*` |

El test `el_tope_interactivo_es_mucho_menor_que_el_del_envelope` no asserta los
números exactos: asserta que **el camino donde hay alguien mirando la pantalla
no comparta tope con el que no**. Ese es el criterio, no el 15.


---

## 5. Nota de entorno para los tests de `nexum-tui`

10 tests de `app::model_panel` y `ui::main_ui::panels::model` **leen el catálogo
vivo del usuario** (`$XDG_DATA_HOME/nexum/providers/provider-catalog-live.json`)
y asumen que no hay ninguno. Con el resolver en default ON el catálogo tiene 56
modelos, así que fallan en una máquina real:

```
assertion `left == right` failed
  left: 56
 right: 0
```

No es una regresión del código: es que esos tests dependen de estado global del
usuario. Con el entorno aislado pasan todos:

```bash
XDG_DATA_HOME=$(mktemp -d) cargo test -p nexum-tui --lib
# test result: ok. 1059 passed; 0 failed
```

**Deuda:** esos tests deberían apuntar a un catálogo de fixture, no al del
usuario. Mientras no se arregle, correr la suite de `nexum-tui` con
`XDG_DATA_HOME` aislado.


---

## 6. Herramientas en modelos locales chicos

### 6.1 El parser tolerante

`nexum-agent/src/llm/tool_call_recovery.rs` destapa bloques de código antes de
dar una respuesta por final: si adentro hay JSON con `name` + argumentos, es un
tool call. Liberal al aceptar (con o sin etiqueta de lenguaje, cerca sin cerrar,
JSON pelado, objeto o array, `arguments`/`parameters`/`input`/`args`, argumentos
como string).

Estricto al emitir: **un nombre que no está en las herramientas disponibles no se
ejecuta jamás.** Sin ese filtro, un modelo explicando cómo se ve un tool call
produciría una ejecución.

### 6.2 Lo que la medición mostró, que no era lo esperado

El caso que originó esto fue `{"name": "ListFiles", "arguments": {}}` de
`qwen2.5:1.5b`. Bien formado y envuelto en markdown — pero **`ListFiles` no es
una herramienta de Nexum.** El inventario real es:

```
AskUserQuestion  Edit  folder_operations  Glob  Grep  Read  TodoWrite  Write
```

El modelo no erró el formato: erró el nombre. Destapar el bloque no rescata ese
caso, y no debe rescatarlo.

En ~10 corridas de `!listá los archivos` contra `qwen2.5:1.5b`, el modelo:

- emitió prosa explicando qué haría (la mayoría),
- alucinó nombres inexistentes (`ListFiles`, `FindAllFiles`),
- una vez pidió `folder_operations` por el camino **estructurado**, que ya
  funcionaba sin este cambio.

**Ninguna corrida produjo el caso que el recuperador rescata.** El parser está
probado de forma determinística (16 unitarios + 3 de cableado en
`react_adapter_test.rs`) y sirve para el modelo que acierta el nombre y lo
envuelve. Pero el cuello de botella medido en modelos de 1.5B no es el formato:
es que no saben qué herramientas tienen.

### 6.3 Capacidad declarada por modelo

Para lo que el parser no salva. `reconcile` consulta `/api/show` de Ollama y
publica `model_capabilities` en el catálogo; `model_tool_support_any()` lo lee y
el turno se rechaza antes de la primera vuelta.

Medido en esta máquina: los 7 modelos qwen/minicpm declaran `tools`;
`moondream:latest` no. Un turno con herramientas contra `moondream` pasó de
**2m 09s de molienda a 0,034s con mensaje legible**.

Tres estados, no dos: `Unknown` no bloquea. No saber no es lo mismo que no
poder — si la ausencia de dato bloqueara, todo provider sin `/api/show` se
quedaría sin herramientas.
