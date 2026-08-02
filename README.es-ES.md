

# swarm

**Orquesta enjambres (swarms) multiagente en Rust puro.**

`swarm` enruta tareas a agentes de código especializados y los compone en flujos de trabajo de orden superior: expansión paralela (fan-out), discusión estructurada y síntesis de un gestor. Es ligero en dependencias, prioriza lo local y se basa en la configuración mediante descriptores en lugar de elecciones de backend codificadas.

[Website](https://swarm.dech.app) ·
[Pages Preview](https://swarm-v57.pages.dev) ·
[Authoring a Backend](docs/authoring-a-backend.md)

```sh
cargo run -p swarm-cli -- fanout "Review the auth module"
cargo run -p swarm-cli -- discuss "Review the session model"
cargo run -p swarm-cli -- metadirector "Plan the next verified slice"
```

## Diseñado para la Coordinación Compleja de Agentes

Las tareas de un solo agente tienen límites. `swarm` coordina diversos modelos de código mediante roles explícitos, registros de sesión y síntesis de un gestor.

| Patrón | Qué hace |
| --- | --- |
| **fan-out** | Envía la misma tarea a trabajadores independientes y luego entrega sus salidas a un gestor para la síntesis. Útil para perspectivas de arquitectura, implementación y revisión. |
| **discuss** | Ejecuta una o más rondas de razonamiento basado en roles. La sesión produce transcripciones inspeccionables y artefactos de resumen. |
| **manager synthesis** | Utiliza un agente gestor para comparar las salidas de los trabajadores, separar los hechos aceptados de las afirmaciones arriesgadas y devolver un paquete de decisión compacto. |

## Características del Motor

- **Backends basados en descriptores**: conecta agentes mediante configuración TOML, no en el código fuente.
- **Entorno nativo en proceso**: `swarm-manager` proporciona un bucle de agente integrado, registro de proveedores, bóveda de credenciales, configuraciones predeterminadas y herramientas integradas.
- **Capa de servidor MCP**: expone informes, manifiestos, sesiones, eventos, transcripciones y superficies de envío a través del crate MCP.
- **Valores predeterminados ligeros en dependencias**: sin runtime asíncrono, cliente HTTP ni TLS a menos que optes por las banderas de características que los requieren.
- **Sesiones inspeccionables**: revisa trabajos anteriores mediante `sessions`, `events`, `transcript` y `overview`.

## Backends Basados en Descriptores

Los backends son descriptores ordinarios. Un agente CLI, una API alojada o un proveedor nativo en proceso pueden seleccionarse todos mediante las mismas reglas de enrutamiento:

```toml
[backend.codex]
kind        = "cli"
command     = "codex"
args        = ["exec", "--model", "{model}"]
prompt      = "stdin"
stream      = "stdout-lines"
ready_check = { binary = "codex" }

[routes.implementation]
preferred = ["codex", "claude:sonnet"]
```

Hay tres tipos de backends:

| Tipo | Qué envuelve |
| --- | --- |
| `cli` | Cualquier agente de línea de comandos, ejecutado como un subproceso. |
| `openai-compatible` | Cualquier punto final HTTP que use `/v1/chat/completions`. |
| `native` | El entorno `swarm-manager` nativo en proceso. |

La mayoría de las integraciones deberían ser solo descriptores. Recurre a Rust solo cuando necesites un protocolo de enlace personalizado, un protocolo de streaming específico, autenticación no estándar o una orquestación multipaso que un descriptor no pueda expresar.

## Dos capas de habilidades (skills)

La palabra "skill" (habilidad) aparece en dos lugares aquí. Son **capas separadas que nunca se tocan**: una moldea a un trabajador, la otra impulsa el motor.

**1. Habilidades que usa el agente.** Los archivos `SKILL.md` colocados en `~/.swarm/skills/<name>/` (o, por proyecto, `<project>/.swarm/skills/<name>/`, que anula el directorio home por nombre) inyectan guía de prompts en un trabajador *nativo* y limitan su conjunto de herramientas a la unión de las `allowed-tools` de cada habilidad. Un backend nativo los selecciona en la configuración:

```toml
[backend.local]
kind     = "native"
provider = "api"
skills   = ["reviewer", "doc"]
```

Inspecciona lo que es cargable con `swarm skills list` (imprime el nombre, descripción, ruta de origen y herramientas permitidas de cada habilidad).

**2. Una habilidad para impulsar el swarm.** [`skills/using-swarm/SKILL.md`](skills/using-swarm/SKILL.md) es la dirección opuesta: una guía que enseña a un agente *host* (Claude Code, Codex, Gemini CLI, etc.) cómo orquestar el CLI de `swarm`: qué verbo elegir, las banderas reales y cómo leer los resultados. Cópialo en el directorio de habilidades de tu agente host. Nunca se inyecta en un trabajador; solo rige cómo se lanza el orquestador.

Las dos capas no interactúan: la capa 1 vive dentro del prompt del sistema de un trabajador, la capa 2 vive en el agente host que llama al binario.

## Arquitectura Modular de Crates

```text
crates/
  swarm-contracts   wire-stable contract types: ids, events, jobs, telemetry
  swarm-core        repository traits and pure domain substrate
  swarm-store       filesystem-backed jobs, sessions, telemetry, and ledgers
  swarm-kernel      stateless routing, config, backend ABI, and classification
  swarm-exec        executor, orchestration, synthesis, sessions, and monitors
  swarm-mcp         MCP server layer, schemas, manifests, reports, dispatch
  swarm-cli         command parsing and CLI command dispatch
  swarm-manager     native single-agent harness, providers, vault, tools
  swarm-registrar   optional generic JSON service-registry hook
```

El workspace mantiene separadas las preocupaciones de contratos, almacenamiento, enrutamiento, ejecución, transporte y agentes nativos para que el motor siga siendo fácil de incrustar, probar y extender.

## Inicio Rápido

Compila el workspace:

```sh
cargo build --release
cargo test
```

Crea una configuración:

```sh
mkdir -p ~/.swarm
cp examples/config.example.toml ~/.swarm/config.toml
```

Define descriptores de backend en `~/.swarm/config.toml`:

```toml
[backend.claude]
kind        = "cli"
command     = "claude"
args        = ["--print", "--model", "{model}"]
prompt      = "stdin"
ready_check = { binary = "claude" }

[settings]
default_agent = "claude"
```

Ejecuta una discusión estructurada:

```sh
cargo run -p swarm-cli -- discuss \
  --participant architecture=claude:sonnet \
  --participant review=codex \
  "Analyze auth.rs for timing vulnerabilities"
```

Sin configuración, el motor usa valores predeterminados integrados y resuelve los CLIs públicos `claude` y `codex` cuando están instalados.

En una instalación nueva, `swarm doctor` verifica toda la configuración en un solo paso: análisis de configuración, estado de lista de cada backend, cadenas de enrutamiento que apuntan a la nada y estado de las credenciales del proveedor. Sale con código distinto de cero si algo bloquearía una ejecución. Para agentes CLI, "listo" solo significa que se encontró el binario: `swarm doctor --probe` envía adicionalmente una pequeña solicitud real a cada agente CLI para verificar que está autenticado en esta máquina.

### Proveedores y credenciales

Las configuraciones de proveedor almacenadas (para backends `native`) residen en un registro cifrado bajo `~/.swarm/providers`. Las claves API se cifran en reposo con una clave maestra almacenada en el gestor de claves del sistema operativo; cuando no hay gestor de claves disponible, las claves nunca se escriben en disco y se leen en tiempo de ejecución desde la variable de entorno `SWARM_PROVIDER_KEY_<ID>` en su lugar.

```sh
swarm provider add api --type openai --models gpt-5.5
pbpaste | swarm provider key set api    # key is read from stdin — never argv
swarm provider list                      # id, type, endpoint, models, key status
swarm provider models openai             # suggested model ids (verify against provider docs)
swarm provider key check api
```

`key set` nunca acepta la clave como argumento de línea de comandos (argv filtra hacia listas de procesos e historial de shell) y nunca la vuelve a mostrar: el único reconocimiento es una longitud enmascarada. `--from-env VAR` copia una clave desde una variable de entorno hacia la bóveda.

## Comandos Principales

| Comando | Úsalo para |
| --- | --- |
| `run` | Ejecutar una sola tarea enrutada. |
| `fanout` / `swarm` | Enviar una tarea a trabajadores paralelos y sintetizar los resultados. |
| `discuss` | Ejecutar una discusión estructurada con múltiples participantes. |
| `metadirector` | Pedirle a un agente gestor que planifique o sintetice a partir del contexto proporcionado. |
| `mcp` | Iniciar la capa de servidor MCP. |
| `sessions`, `events`, `transcript`, `overview` | Inspeccionar trabajos anteriores y salidas en tiempo de ejecución. |
| `scaffold-backend` | Generar un descriptor y un esqueleto de trait en Rust para un nuevo backend. |
| `skills` | Listar las habilidades `SKILL.md` que un backend nativo puede cargar (`swarm skills list`). |
| `provider` | Gestionar proveedores almacenados y sus credenciales (bóveda + respaldo por env). |
| `doctor` | Verificar salud de configuración, backends, enrutamiento y credenciales de proveedores. |

## Banderas de Características (Feature Flags)

| Característica | Crate | Qué añade |
| --- | --- | --- |
| `openai` | `swarm-exec` | Backend HTTP compatible con OpenAI mediante `ureq` y rustls. |
| `native` | `swarm-exec` | Backend de agente único en proceso a través de `swarm-manager`. |
| `runtime` | `swarm-manager` | Bucle asíncrono de agente y herramientas integradas exec/archivo. |
| `http` | `swarm-manager` | Proveedores HTTP y herramienta web a través de `reqwest` y rustls. |
| `registry` | `swarm-mcp` | Autorregistro opcional en registro de servicios JSON. |
| `rmcp` | `swarm-mcp` | Transporte MCP basado en rmcp. |

## Modelo de Amenazas y Límites de Seguridad

`swarm` es un orquestador, no una sandbox (entorno aislado).

- Un backend `cli` ejecuta el comando en tu configuración con los privilegios de tu usuario.
- Los prompts y salidas respaldados por API salen de tu máquina y van al punto final que configures.
- Los CLIs de backend mantienen sus propios sistemas de permisos, si los tienen. `swarm` no media el acceso a archivos o red en la v1.
- `swarm-manager` puede cifrar credenciales en reposo, pero los backends de subproceso aún pueden leer lo que sus permisos de proceso permitan.

Trata la configuración del backend como un perfil de shell: revísala, mantén los secretos fuera del repositorio y solo ejecuta agentes que confíes en directorios a los que estés dispuesto a exponer.

## Desarrollo

```sh
cargo fmt --all
cargo test
cargo test -p swarm-cli
cargo test -p swarm-manager --features http
```

La superficie de comandos del CLI está protegida por pruebas porque los wrappers externos dependen de tokens de comando exactos. Al cambiar el nombre de un comando, actualiza la guardia de estabilidad intencionalmente.

## Licencia

Licenciado bajo cualquiera de [MIT](LICENSE-MIT) o
[Apache-2.0](LICENSE-APACHE), a tu elección.
