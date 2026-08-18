# ============================================================
# Nexum TUI — Español rioplatense (es-AR)
# Traducción completa del archivo en/main.ftl
# ============================================================

# ---- i18n infrastructure test keys ----
test-hello = ¡Hola, Mundo!
test-greeting = ¡Hola, { $name }!
ui-empty = (ninguno)

# ---- Command Descriptions ----

command-help-description = Listar todos los comandos disponibles
command-clear-description = Limpiar la lista de mensajes
command-exit-description = Salir de la aplicación
command-compact-description = Comprimir contexto de conversación (resumen estructurado + reinserción de archivos/Skills recientes)
command-model-description = Abrir panel de selección de modelo (provider + modelo real + thinking); con argumento cambia a un modelo disponible del provider activo
command-login-description = Gestionar configuración de Providers (crear/editar/borrar)
command-cost-description = Ver costo de la sesión actual y consumo de tokens
command-context-description = Ver uso de contexto y estadísticas de la sesión
command-agents-description = Abrir panel de selección de Agent
command-mcp-description = Gestionar conexiones de servidores MCP
command-memory-description = Editar archivos de memoria CLAUDE.md de usuario/proyecto
command-history-description = Abrir navegador de historial de conversaciones
command-loop-description = Registrar tarea de bucle programado (descripción en lenguaje natural, ej. /loop recordame tomar agua cada 5 minutos)
command-cron-description = Ver y gestionar tareas programadas
command-tasks-description = Ver hilos de agente y tareas programadas
command-plugin-description = Gestionar plugins (navegar, instalar, desinstalar)
command-config-description = Configuración global (autocompact, idioma, sobreescritura de system prompt)
command-hooks-description = Ver configuración de Hooks
command-effort-description = Ver o ajustar nivel de razonamiento (low/medium/high/xhigh/max)
command-rename-description = Ver o modificar el título de la sesión actual
command-lang-description = Cambiar idioma de la interfaz (ej. /lang es-AR)
command-setup-description = Abrir asistente de configuración para configurar Providers
command-agent-description = Definir Agent, cambiar entre diferentes roles de Agent

# ---- Command Execution Messages ----

# help command
help-available-commands = Comandos disponibles:
help-alias-prefix = (alias: /{ $aliases })
help-skills-count = Skills ({ $count } disponibles): escribí # para ver
help-skills-empty = Skills: poné archivos .md en el directorio .claude/skills/ para agregar
help-shortcuts = Atajos: Shift+Tab alternar modo de permisos | { $model_key } cambiar modelo | Shift+Enter nueva línea | Esc salir | Ctrl+C interrumpir

# compact command
compact-agent-running = El agente está ejecutándose, no se puede comprimir

# history command
history-agent-running = El agente está ejecutándose, no se puede abrir el historial

# model command
config-save-failed = Error al guardar configuración: { $error }

# effort command
effort-set = Nivel de razonamiento ajustado a { $effort }
effort-current = Nivel de razonamiento actual: { $effort }
effort-usage = Uso: /effort low|medium|high|xhigh|max

# loop command
loop-usage = Uso: /loop <descripción temporal en lenguaje natural> <prompt>
loop-example = Ejemplo: /loop recordame tomar agua cada 5 minutos

# rename command
rename-no-session = No hay sesión activa, no se puede renombrar
rename-current-title = Título actual: { $title }
rename-updated = Título de sesión actualizado a: { $name }
rename-failed = Error al renombrar: { $error }
rename-untitled = (sin título)

# lang command
lang-switched = Idioma cambiado a { $lang }
lang-available = Idiomas disponibles: { $langs }
lang-unsupported = Idioma no soportado: { $lang }

# ---- Status Bar ----

statusbar-permission-dont-ask = No Preguntar
statusbar-permission-accept-edit = Aceptar Edit
statusbar-permission-auto = Auto Mode
statusbar-permission-bypass = Bypass
statusbar-copied =  { $count } caracteres copiados
statusbar-no-response-to-copy = no hay respuesta para copiar todavía

copy-response-button = Copiar
copy-response-copied = Copiado

statusbar-no-agent = Ninguno
statusbar-bg-indicator = [BG: { $count }]
statusbar-retrying = Reintento { $attempt }/{ $max } ({ $delay }s): { $error }
statusbar-mcp-connecting =  MCP ({ $connected }/{ $total })...
statusbar-mcp-ready =  MCP listo ({ $total } servidores)
statusbar-mcp-failed =  MCP falló: { $msg }
statusbar-lsp-diag = diag: { $errors }E/{ $warnings }W

# ---- Status Bar Shortcut Hints (main view) ----

key-command = :Comando
key-switch-session = :Cambiar Sesión
key-close = :Cerrar
key-scroll = :Desplazar
key-cancel = :Cancelar
key-newline = :NuevaLínea
key-open-browser = :Abrir navegador
key-submit = :Enviar
key-switch = :Cambiar
key-switch-tab = :CambiarPestaña
key-move = :Mover
key-select = :Seleccionar
key-confirm = :Confirmar
key-delete = :Borrar
key-reconnect = :Reconectar
key-detail = :Detalle
key-execute = :Ejecutar
key-back = :Volver
key-install = :Instalar
key-tab = :Pestaña
key-effort = :Esfuerzo
key-switch-model = :CambiarModelo

# ---- Welcome Page ----

welcome-title = AI Operating System Personal
welcome-intro = Estoy listo para ayudarte a desarrollar, analizar y coordinar tareas técnicas.
welcome-feature-code = Desarrollar: escribir, refactorizar y corregir código
welcome-feature-explore = Explorar: buscar en el codebase y entender arquitectura
welcome-feature-document = Documentar: crear reportes, issues y notas técnicas
welcome-feature-analyze = Analizar: revisar logs, errores y calidad de código
welcome-feature-investigate = Investigar: buscar información cuando el provider lo permita
welcome-login-hint-1 = Escribí
welcome-login-hint-2 = o
welcome-shortcut-quit = :Salir
welcome-shortcut-stop = :Parar
welcome-shortcut-newline = :NuevaLínea
welcome-shortcut-mode = :Modo
welcome-shortcut-model = :Modelo
welcome-skills-available = { $count } skills disponibles

# ---- Tips (18 items) ----

tip-0 = Escribí / para comandos, Tab para autocompletar
tip-1 = Ctrl+C interrumpe al Agent, Shift+Tab cambia modo de permisos
tip-2 = Ctrl+T cambia entre modelos disponibles, Ctrl+Shift+T cambia provider
tip-3 = Shift+Enter para nueva línea en el input
tip-4 = Arrastrá archivos o imágenes a la terminal para adjuntarlos al mensaje
tip-5 = Mantené Ctrl+V para pegar imagen del portapapeles
tip-6 = Ctrl+U/D desplazan historial de mensajes, Up/Down navegan historial de input
tip-7 = Ctrl+N/P cambian de Sesión, Ctrl+W cierra
tip-8 = Esc cierra popup o panel, Enter confirma selección
tip-9 = /compact comprime contexto para ahorrar tokens
tip-10 = /clear limpia la conversación actual
tip-11 = /model cambia el modelo LLM
tip-12 = /history navega el historial de conversaciones
tip-13 = /loop crea tareas de bucle programadas
tip-14 = /plugin gestiona plugins
tip-15 = Agregá Skills personalizados en .claude/skills/
tip-16 = Definí SubAgents en .claude/agents/
tip-17 = Para tareas complejas, que el Agent planifique primero antes de ejecutar

# ---- Setup Wizard ----

setup-welcome-title = Conectá un proveedor
setup-choose-provider = Nexum necesita un modelo para responder. Elegí de dónde sale.
setup-source-custom-api = API Personalizada
setup-source-migrate = Importar configuración existente
setup-source-custom-desc = Ingresá los datos del provider manualmente
setup-source-migrate-desc = Importar configuración desde ~/.claude/
setup-key-confirm = elegir
setup-key-select = mover
setup-key-quit = saltar
setup-configure-title = Configurá tus proveedores
setup-submit = Enviar
setup-key-edit-submit = editar / guardar
setup-key-check = marcar
setup-key-back = volver
setup-edit-title =  ── Setup ── Editar: { $type } ({ $id })
setup-field-type = Tipo
setup-field-id = ID
setup-field-base-url = Base URL
setup-hint-base-url-v1 = La Base URL de OpenAI necesita sufijo /v1
setup-field-api-key = API Key
setup-field-opus = Modelo principal
setup-field-sonnet = Modelo balanceado
setup-field-haiku = Modelo rápido
setup-model-label = Modelo
setup-label-key = Key:
setup-provider-anthropic = Anthropic
setup-provider-openai = Compatible con OpenAI
setup-confirm = Confirmar
setup-test-connectivity = [ Probar Conexión ]
setup-key-switch-type = :Cambiar tipo
setup-key-back-list = :Volver al listado
setup-complete-title = Listo
setup-press-enter = Presioná
setup-to-start = para empezar a usar
setup-no-key = (sin key)
setup-no-providers = No hay providers configurados. Agregá uno seleccionando "API Personalizada" o importando una configuración existente.

setup-language-title = Elegí tu idioma
setup-language-prompt = Todo lo que viene después va a estar en el idioma que elijas.
setup-language-press-enter = Presioná Enter para confirmar

# ---- Config Panel ----

config-panel-title =  /config — Configuración
config-field-autocompact = Autocompact
config-field-compact-threshold = Compact Threshold
config-field-language = Idioma
config-field-persona = Persona
config-field-tone = Tono
config-field-proactiveness = Proactividad
config-field-cache-warning = Advertencia de Cache
config-field-diff = Diff en línea
config-value-on = ON
config-value-off = OFF
config-saved = Configuración guardada

# Config panel groups
config-group-general = General
config-group-prompt-overrides = Sobreescritura de Prompt

# Config field descriptions
config-desc-autocompact = (ON/OFF — comprimir contexto automáticamente cuando esté lleno)
config-desc-threshold = 50-99% — umbral para activar auto-compact
config-desc-language = en, es-AR, zh-CN, o vacío para auto
config-desc-persona = Sobreescribir persona del system prompt (vacío = default)
config-desc-tone = Sobreescribir tono del system prompt (vacío = default)
config-desc-proactiveness = low / medium / high — nivel de iniciativa del agente
config-desc-cache-warning = (ON/OFF — mostrar advertencia de cache hit rate bajo en el chat)
config-desc-diff = (ON/OFF — mostrar diff en línea para herramientas Write/Edit)
config-field-streaming = Modo Streaming
config-desc-streaming = streaming / block / none — granularidad de renderizado del output del LLM

# ---- Login Panel ----

login-panel-title-browse =  /login — Gestión de Providers
login-panel-title-edit =  /login — Editar Provider
login-panel-title-new =  /login — Nuevo Provider
login-panel-title-confirm-delete =  /login — Confirmar Borrado
login-no-model = (no configurado)
login-empty-hint =   (sin provider, presioná Ctrl+N para crear)
login-confirm-delete-label =  Confirmar borrado
login-confirm-delete-question =  ?
login-key-activate = :Activar
login-key-new = :Nuevo
login-key-delete = :Borrar
login-key-paste = :Pegar
login-confirm-delete = :Confirmar borrado

# ---- HITL Popup ----

hitl-single-title =  ⚠ Aprobación de Herramienta (1 item)
hitl-batch-title =  ⚠ Aprobación por Lote
hitl-approved = [Aprobado]
hitl-rejected = [Rechazado]
hitl-summary = Seleccionados: { $approved } aprobados / { $rejected } rechazados

# ---- AskUser Popup ----

ask-user-placeholder = Escribí algo.

# ---- App Messages ----

app-provider-ready = { $name } ({ $model }) listo
app-not-configured = No configurado
app-empty = Ninguno
app-no-api-key-warning = Atención: No hay API Key configurada (ANTHROPIC_API_KEY o OPENAI_API_KEY)
app-interrupted-resumed = Forzado a interrumpir
app-interrupt-done = Interrumpido
app-interrupted-background = Forzado a interrumpir
app-config-saved = Configuración guardada
app-config-save-failed = Error al guardar configuración: { $error }
app-provider-activated = Provider activado: { $name }
app-provider-created = Provider creado y activado: { $name }
app-provider-saved = Provider guardado y activado: { $name }
app-provider-deleted = Provider borrado: { $name }
app-provider-name-empty = Error al guardar: el nombre del Provider no puede estar vacío
app-agent-reset = Agent reseteado (sin agent_id configurado)
app-agent-switched = Agent cambiado a: { $name } ({ $id })
app-agent-disconnected = Conexión con el Agent perdida, reintentá enviar
app-compact-no-context = No hay contexto comprimible (el historial está vacío)
app-compact-no-provider = Compact falló: No hay LLM Provider configurado (configurá ANTHROPIC_API_KEY o OPENAI_API_KEY)
app-compact-compressing = Comprimiendo contexto
app-compact-done = Contexto comprimido
app-compact-failed = Compact falló: { $error }
app-compact-auto-cleared = Limpieza automática: { $count } resultados de herramientas liberados
app-compact-limit-reached = El contexto sigue excediendo el límite después de comprimir. Usá /compact para comprimir manualmente o /clear para limpiar el historial.
app-model-switched = Modelo activo: { $alias } ({ $effort } effort)
app-1m-context-enabled = Modo contexto 1M activado (ventana de contexto: 1,000,000 tokens)
app-prompt-cache-low = Cache hit rate { $rate }% < 80% (req: { $req })
app-no-mcp-configured = No hay servidores MCP configurados (agregalos en .mcp.json o settings.json)
app-no-cron-tasks = No hay tareas cron
app-cron-deleted = Tarea cron borrada: { $preview }
app-submit-attachments = { $input } [{ $count } imagen(es)]
app-no-provider-submit = No hay API Key configurada, escribí /login para configurar un Provider
app-bg-task-done = [Tarea de fondo { $id } completada] Agent: { $agent } | Llamadas a herramientas: { $tools } | Duración: { $duration }ms
app-bg-task-done-with-result = [Tarea de fondo { $id } completada] Agent: { $agent } | Llamadas a herramientas: { $tools } | Duración: { $duration }ms\nResultado:\n{ $result }
app-bg-task-failed = [Tarea de fondo { $id } falló] Agent: { $agent } | { $error }
app-bg-task-failed-with-error = [Tarea de fondo { $id } falló] Agent: { $agent }\nError:\n{ $error }
app-bg-continuation = Revisando { $count } resultado(s) de agente de fondo...

# ---- Panel Status Bar Hints ----

# Login panel
hint-login-browse = :Navegar
hint-login-activate = :Activar
hint-login-edit = :Editar
hint-login-new = :Nuevo
hint-login-delete = :Borrar
hint-login-close = :Cerrar
hint-login-field = :Campo
hint-login-save = :Guardar
hint-login-paste = :Pegar
hint-login-toggle = :Alternar
hint-login-back = :Volver

# Config panel
hint-config-field = :Campo
hint-config-toggle = :Alternar
hint-config-save = :Guardar y cerrar

# Model panel
hint-model-navigate = :Navegar
hint-model-confirm = :Confirmar
hint-model-effort = :Esfuerzo
hint-model-close = :Cerrar

# Agent panel
hint-agent-select = :Seleccionar
hint-agent-confirm = :Confirmar
hint-agent-cancel = :Cancelar

# MCP panel
hint-mcp-navigate = :Navegar
hint-mcp-detail = :Detalle
hint-mcp-reconnect = :Reconectar
hint-mcp-delete = :Borrar
hint-mcp-execute = :Ejecutar
hint-mcp-back = :Volver
hint-mcp-close = :Cerrar

# ---- MCP Panel Content ----

mcp-server-count = { $count } servidores
mcp-section-project = MCPs del Proyecto
mcp-section-project-path = MCPs del Proyecto ({ $path })
mcp-section-user = MCPs de Usuario
mcp-section-user-path = MCPs de Usuario ({ $path })
mcp-section-plugin = MCPs de Plugin
mcp-no-servers = No hay servidores MCP configurados. Editá .mcp.json o settings.json
mcp-panel-title = Gestionar servidores MCP
# Status
mcp-status-connected = conectado
mcp-status-needs-auth = necesita autenticación
mcp-status-error = error
mcp-status-disabled = deshabilitado
mcp-status-uninitialized = no inicializado
mcp-status-offline = offline
# Auth
mcp-auth-authenticated = autenticado
mcp-auth-none = ninguna
# Labels
mcp-label-status = Estado:
mcp-label-auth = Auth:
mcp-label-url = URL:
mcp-label-config-location = Ubicación de configuración:
mcp-label-plugin = Plugin
mcp-label-plugin-source = Plugin - { $source }
mcp-label-capabilities = Capacidades:
mcp-label-tools = Herramientas:
mcp-label-tools-count = { $count } herramientas
# Capabilities
mcp-capability-tools = herramientas
mcp-capability-resources = recursos
# Actions
mcp-action-hide-tools = Ocultar herramientas
mcp-action-view-tools = Ver herramientas
mcp-action-reauthenticate = Re-autenticar
mcp-action-clear-auth = Limpiar autenticación
mcp-action-reconnect = Reconectar
mcp-action-disable = Deshabilitar
mcp-action-enable = Habilitar
# OAuth Messages
mcp-oauth-completed = [i] Autorización OAuth completada: { $server }
mcp-oauth-failed = [i] Autorización OAuth falló: { $server } - { $error }
mcp-clear-auth-ok = [i] Credenciales OAuth limpiadas: { $server }
mcp-clear-auth-failed = [i] Error al limpiar credenciales OAuth: { $server }
mcp-action-ok = [i] Acción completada: { $server }
mcp-action-failed = [i] Acción falló: { $server }

# Plugin panel
hint-plugin-uninstall = :Confirmar desinstalación
hint-plugin-cancel = :Cancelar
hint-plugin-delete = :Confirmar borrado
hint-plugin-add = :Agregar
hint-plugin-exit-search = :Salir de búsqueda
hint-plugin-tab = :Pestaña
hint-plugin-install = :Instalar
hint-plugin-remove = :Quitar
hint-plugin-navigate = :Navegar
hint-plugin-execute = :Ejecutar
hint-plugin-back = :Volver al listado
hint-plugin-select = :Seleccionar
hint-plugin-search = :Buscar

# Cron panel
hint-cron-confirm-delete = :Confirmar borrado
hint-cron-navigate = :Navegar
hint-cron-toggle = :Alternar
hint-cron-delete = :Borrar
hint-cron-close = :Cerrar

# Status panel
hint-status-tab = :CambiarPestaña
hint-status-close = :Cerrar

# History panel
hint-history-confirm-delete = :Confirmar borrado
hint-history-exit-search = :Salir de búsqueda
hint-history-close = :Cerrar

# Hooks panel
hint-hooks-navigate = :Navegar
hint-hooks-close = :Cerrar

# Memory panel
hint-memory-select = :Seleccionar
hint-memory-edit = :Editar
hint-memory-close = :Cerrar

# ---- Plugin Panel Messages ----

app-plugin-updating = Actualizando marketplace: { $name }
app-plugin-delete-failed = Error al borrar: { $error }
app-plugin-add-failed = Error al agregar: { $error }
app-plugin-added = Marketplace agregado: { $name } (obteniendo contenido...)

# Background Agent Bar
bg-bar-focus-hint = Presioná Esc para salir del foco

# ---- Model Panel ----

model-panel-title =  Seleccionar modelo 
model-panel-description =   Cambiá entre modelos. Aplica a esta sesión.
model-field-max-token = Max Token
model-field-effort = Esfuerzo
model-field-1m-context = Contexto 1M
model-effort-low = Bajo
model-effort-medium = Medio
model-effort-high = Alto
model-effort-xhigh = XAlto
model-effort-max = Máximo

# ---- Status Panel ----

status-panel-title =  Estado 
status-tab-cost = Costo
status-tab-context = Contexto
status-label-duration = Duración de Sesión
status-label-input-tokens = Tokens de Entrada
status-label-output-tokens = Tokens de Salida
status-label-cache-create = Creación de Cache
status-label-cache-read = Lectura de Cache
status-label-llm-calls = Llamadas LLM
status-label-estimated-cost = Costo Est.
status-label-current-model = Modelo Actual
status-label-context = Contexto
status-label-used = Usado
status-label-messages = Mensajes
status-label-tools = Herramientas
status-empty-data = Sin datos de solicitud

# ---- Agent Panel ----

agent-panel-title-none =  Seleccionar Agent (Ninguno) 
agent-panel-title =  Seleccionar Agent 
agent-panel-none-label = Sin Agent (default)
agent-panel-empty-hint = Agregá archivos de definición de Agent en .claude/agents/

# ---- Hooks Panel ----

hooks-panel-title-none =  Hooks (ninguno configurado) 
hooks-panel-title =  Hooks 
hooks-configured-count = { $count } hooks configurados
hooks-readonly-hint = Este panel es de solo lectura. Para agregar o modificar hooks, editá plugin hooks.json.
hooks-no-hooks =   No hay hooks configurados.
hooks-no-hooks-hint =   Se pueden agregar hooks via plugin hooks/hooks.json.

# ---- Thread Browser ----

thread-browser-title =  Reanudar Sesión ({ $cursor }/{ $total }) 
thread-browser-search-placeholder = Buscar…
thread-browser-empty =   (No hay conversaciones todavía)
thread-browser-no-match =   (No hay conversaciones que coincidan)
thread-browser-untitled = (sin título)
thread-browser-time-just-now = justo ahora
thread-browser-time-minutes = hace { $count } minuto{ $suffix }
thread-browser-time-hours = hace { $count } hora{ $suffix }
thread-browser-time-days = hace { $count } día{ $suffix }

# ---- Rewind Popup ----

rewind-title = Rewind
rewind-msg-count = ({ $count }msg)
rewind-mode-messages = 1. Volver a este prompt
rewind-mode-files = 2. Volver a este prompt + restaurar archivos
rewind-mode-confirm = ⚠ ¿Confirmar: restaurar archivos?
rewind-files-to-restore = Archivos a restaurar:
rewind-confirm-hint = Enter para confirmar, Esc para cancelar
rewind-write-op = Write → Borrar + Git restore
rewind-edit-op = Edit → Restaurar

# ---- OAuth Popup ----

oauth-title =  Autorización OAuth — { $server } 
oauth-prompt = Presioná Ctrl+O para abrir en el navegador, después pegá la URL de callback:
oauth-callback-label = URL de Callback > 

# ---- Login Panel ----

login-field-name = Nombre
login-field-type = Tipo
login-field-base-url = Base URL
login-field-api-key = API Key
login-field-opus-model = Modelo principal
login-field-sonnet-model = Modelo balanceado
login-field-haiku-model = Modelo rápido

# ---- Config Panel additional ----

config-lang-display-en = English
config-lang-display-zh = 简体中文
config-lang-display-es = Español
config-lang-display-auto = auto
config-streaming-display-streaming = streaming
config-streaming-display-block = block
config-streaming-display-none = none
config-proactiveness-display-low = bajo
config-proactiveness-display-medium = medio
config-proactiveness-display-high = alto

# ---- Command Outputs ----

command-channel-desc = Gestionar conexiones de canal MCP: open <source> / close / status
command-channel-usage = Uso: /channel open <source> | /channel close | /channel status
command-channel-not-init = Sistema de canales no inicializado
command-channel-unavailable = El servidor { $server } no soporta canales o no está conectado
command-channel-opened = Canal abierto: { $source }
command-channel-all-closed = Todos los canales cerrados
command-channel-closed = Canal cerrado: { $server }
command-channel-no-channels = No hay canales abiertos. Usá /channel open <source> para abrir
command-channel-list-header = Canales abiertos:
command-channel-list-item =   { $source }
command-bg-usage = Uso: /bg <descripción del comando>
    Ejemplo: /bg Buscá el roadmap de Rust 2026 en español
command-loop-usage = Uso: /loop <tiempo en lenguaje natural> <prompt>
    Ejemplo: /loop recordame tomar agua cada 5 minutos
command-plugin-add-failed-detail = Error al agregar marketplace: { $error }
command-plugin-install-failed = Error al instalar plugin: { $error }
command-plugin-update-failed = Error al actualizar marketplace: { $error }
command-agent-reset = Agent reseteado (sin agent_id configurado)
command-agent-switched = Agent cambiado a: { $name } ({ $id })
command-lang-current-suffix =  (actual)
command-config-save-failed = Error al guardar config: { $error }
command-plugin-help = Uso:
    /plugin                                    — Abrir panel de plugins
    /plugin marketplace add <url>              — Agregar fuente de marketplace
    /plugin install <name>@<marketplace>       — Instalar plugin
    /plugin marketplace update <name>          — Actualizar cache de marketplace

# ---- Message Rendering ----

render-batch-all-failed = { $count } agents fallaron
render-batch-partial = { $done } agents terminaron, { $failed } fallaron
render-batch-done = { $count } agents terminaron
render-status-failed = Falló
render-status-done = Hecho
render-tool-uses = · { $count } usos de herramientas
render-user-answered = El usuario respondió las preguntas de Nexum:
render-thought-for = Pensó durante { $count } caracteres
render-agent-header = Agent

# ---- Message Area Spinner ----

msg-spinner-tokens = · ↓ { $count } tokens
msg-spinner-brewed =   ✻  Trabajó durante { $duration }
msg-tip-prefix =   ⎿  Tip: 
msg-todo-available =  (disponible)

# ---- Message View Placeholders ----

msg-placeholder-image = [Imagen]
msg-placeholder-document = [Documento: { $name }]

# ---- App Misc ----

app-cli-no-input = No hay prompt de entrada. Uso: nexum -p "tu pregunta" o echo "pregunta" | nexum -p
app-thread-deleted = Conversación borrada: { $title }
app-memory-project = Descripción del Proyecto
app-memory-user = Usuario Global

# ---- Status Bar additional ----

statusbar-rewind-wait =  El Agent está ejecutándose, esperá antes de hacer rewind
statusbar-rewind-pending =  Presioná ESC de nuevo para hacer rewind
statusbar-rewind-action = :Rewind
statusbar-rewind-other-key = :Otras teclas
statusbar-rewind-move = :Mover
statusbar-rewind-switch-file = :Cambiar archivo a restaurar
