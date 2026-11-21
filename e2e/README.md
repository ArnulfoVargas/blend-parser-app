# Pruebas de punta a punta

Corren la **app real** (backend en Rust, importación con Blender, interfaz en
WebKitGTK) con [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/)
y WebdriverIO. Solo en Linux.

## Preparar

```sh
sudo apt install webkit2gtk-driver xvfb blender python3-numpy
cargo install tauri-driver --locked
cargo build -p blend-parser-app
cd e2e && npm install && npm run fixtures
```

`npm run fixtures` genera con Blender `fixtures/out/figure.blend` (figura pintada,
21k triángulos). Con `-- fixtures/out 4` sale `figure-s4.blend` (257k), para medir.
`fixtures/simple.py` genera un cilindro cerrado y una cabeza abierta, para separar
problemas de geometría no cerrada.

## Correr

| Comando | Qué hace |
|---|---|
| `node run.mjs [escenario…]` | Flujo normal: abrir, tamaño, corte, malla, viabilidad, pincel, exportar, accesibilidad, modos, inglés, y las herramientas de Malla: reparaciones, orientación, vistas de viabilidad, grosor, simplificar protegido, suavizar, vaciar, engrosar y las ayudas del visor (con `cylinder.blend`, `head.blend` y `thin.blend` de `simple.py`). |
| `node break.mjs [ataque…]` | Intenta romper la app: argumentos hostiles a cada comando, ráfagas concurrentes, deshacer en bucle, cortar todo, basura en cada campo numérico, clics repetidos. Tras cada ataque comprueba que el documento sigue respondiendo (un *panic* con el documento bloqueado lo deja inservible) y que la ventana no registró errores. |
| `node probe-cut.mjs <modelo>` / `probe-axis.mjs` | Dónde se rechaza un corte plano / por eje, del 5 % al 95 %. |

Variables: `BP_APP` (binario), `BP_ROOT` (checkout cuya interfaz sirve Vite;
sirve para probar el worktree de una PR), `BP_OUT` (capturas e informes,
`out/` por defecto), `BP_MODEL`, `BP_PORT` y `BP_DISPLAY` (para correr dos a la
vez), `BP_EMBEDDED=1` para un binario con la interfaz dentro
(`cargo build -p blend-parser-app --features tauri/custom-protocol`), que no
necesita Vite.

Cada corrida deja capturas `.png` y un `report.json` / `break.json` con los
resultados y la duración de cada comando visto desde la interfaz.

## Medir el backend

```sh
cargo run --release -p blend-parser-app --features bench --example bench -- e2e/fixtures/out/figure-s4.blend 5
```

Tabla con el coste de cada comando (operación + resumen que devuelve) y de las
etapas internas que se repiten (`Mesh::build`, `adjacency`, `Bvh::build`,
`paint::build`, `small_islands`…).
