# BlendParser

App de escritorio (Tauri 2 + Rust) que convierte un `.blend` con materiales y
texturas en un `.3mf` que Bambu Studio abre **ya pintado por filamento**.

- Carga por arrastrar y soltar, por el diálogo o con "Abrir con…".
- Materiales a pintura por triángulo (`paint_color`), con bordes subdivididos como hace Bambu.
- Colores objetivo editables por ranura del AMS, o sugeridos de nuevo con 2 a 12 filamentos.
- Reducción de colores por cercanía (ΔE en Lab): de 10 a 5 uniendo los más parecidos, o
  unión manual de una ranura a otra. Las uniones re-etiquetan la pintura (respetan lo pintado
  a mano) y persisten aunque se recalcule.
- Vista previa 3D: orbitar, mover y acercar; vista de filamentos o de textura original.
- Pincel sobre la malla con los colores objetivo, con deshacer (`Ctrl+Z`).
- Tamaño final, con aviso y ajuste a la placa de la impresora elegida.
- Optimizaciones para imprimir más rápido: limpiar islas de color, nivel de detalle de bordes,
  simplificar la malla y aplanar la base, con estimación de cambios de filamento y purga.
- Reparación de malla: une vértices duplicados, quita triángulos degenerados y caras repetidas,
  y elimina piezas sueltas menores que un tamaño dado.
- Exportar por color: un `.zip` con un STL cerrado por color (cáscara del grosor elegido hacia
  dentro; opcionalmente el color principal como sólido completo) y un `colores.txt`.

## Descarga

Los instaladores de Windows (`.exe`, `.msi`) y Linux (`.AppImage`, `.deb`) están en
[Releases](https://github.com/ArnulfoVargas/blend-parser-app/releases). Hace falta tener
**Blender 3.6 o posterior** instalado.

Los instaladores de Windows se firman con un certificado de
[SignPath Foundation](https://signpath.org) en cuanto el proyecto queda aprobado; hasta entonces
salen sin firmar, y SmartScreen puede pedir "Más información > Ejecutar de todas formas". Aun
firmados, SmartScreen puede avisar en las primeras descargas de una versión mientras el
certificado gana reputación.

**Code signing policy / Política de firma de código:** [CODE_SIGNING.md](CODE_SIGNING.md).
Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

### Activar la firma (mantenedores)

1. Solicitar el proyecto en [signpath.org/apply](https://signpath.org/apply.html) y activar la
   verificación en dos pasos en GitHub y en SignPath.
2. En SignPath: crear el proyecto con el repositorio de GitHub como origen, pegar
   [`.signpath/artifact-configuration.xml`](.signpath/artifact-configuration.xml) como configuración
   de artefactos y crear una política de firma `release-signing` con aprobación manual.
3. En GitHub (Settings > Secrets and variables > Actions): el secreto `SIGNPATH_API_TOKEN` y las
   variables `SIGNPATH_ORGANIZATION_ID` y `SIGNPATH_PROJECT_SLUG` (y `SIGNPATH_SIGNING_POLICY_SLUG`
   si la política no se llama `release-signing`).

Con `SIGNPATH_PROJECT_SLUG` definida, el flujo de release compila los instaladores, espera la
aprobación en SignPath, vuelve a firmarlos para el actualizador y los publica.

## Requisitos para compilar

- **Blender** 3.6 o posterior. La app lo busca en `C:\Program Files\Blender Foundation`,
  `/Applications/Blender.app`, `/usr/bin` o en `PATH`; también se puede indicar con
  `BLENDER_PATH` o desde la app.
- Rust estable (1.88+), Node 20, y en Windows WebView2 (viene con Windows 11).
- Linux: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`.

## Desarrollo

```sh
npm install
npm run tauri dev                    # desarrollo con recarga
npm run tauri build -- --no-bundle   # solo el ejecutable, en target/release
npm run tauri build                  # instaladores de la plataforma actual
```

Antes de cada commit (lo mismo que comprueba CI):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run build
```

Prueba con un modelo real y Blender instalado:

```sh
BP_SAMPLE_BLEND=ruta/al/modelo.blend cargo test --release -p bp-blend -- --ignored --nocapture
```

## Actualizaciones automáticas

Desde la 0.0.2, BlendParser busca una versión nueva al arrancar (y al pulsar la versión en
la barra superior). Lee `latest.json` del último Release de GitHub, comprueba la firma con la
clave pública de `tauri.conf.json`, instala y se reinicia. En Linux se actualiza el AppImage;
`.deb` y `.rpm` se actualizan a mano.

La clave privada de firma está fuera del repositorio, en `~/.tauri/blendparser.key` (con su
contraseña en `~/.tauri/blendparser.key.password`), y en los secretos del repositorio
`TAURI_SIGNING_PRIVATE_KEY` y `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. **Guarda una copia de
seguridad**: sin ella, las instalaciones existentes no aceptarán actualizaciones nuevas.

## Publicar una versión

Cada PR fusionada a `main` publica una versión automáticamente cuando pasan todas las
comprobaciones de CI. Por defecto incrementa el parche (`0.0.2` → `0.0.3`); añade a la PR
la etiqueta `release:minor` o `release:major` antes de fusionarla para subir esos componentes.
Si tiene ambas, gana `release:major`. Un push directo a `main` no publica una versión.

La automatización toma la versión más alta entre los tags `alpha-*`, `beta-*`, `v*` y la
versión del código. Actualiza los manifiestos y lockfiles en un commit de release derivado
del commit que pasó CI, y sube solamente tags: **no escribe en `main`** ni necesita un PAT.
La versión en `main` puede quedar por debajo de la publicada; la versión distribuida está
en su tag `alpha-X.Y.Z`. El tag `release-source-<sha>` permite reintentar el mismo commit
sin crear otra versión. Las reservas de tags son atómicas para evitar colisiones.

El workflow reutilizable de Release se invoca directamente desde CI, porque los tags
creados con `GITHUB_TOKEN` no disparan otro workflow. Compila y firma Windows y Linux,
valida que `latest.json` contenga ambas plataformas y entonces publica el borrador.
Si falla una build, el release queda como borrador y la versión anterior sigue disponible.
Reintenta con **Re-run all jobs** en CI o con **Actions → Release → Run workflow**, indicando
el tag existente. Un release ya publicado se omite. No borres los tags de seguimiento.

Los secretos `TAURI_SIGNING_PRIVATE_KEY` y `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` deben seguir
configurados en GitHub. El workflow solicita `contents: write`; no requiere permitir que
Actions apruebe PRs ni cambiar las protecciones de `main`.

Para crear una versión manualmente:

```sh
npm run release -- 0.0.3 --push
```

El script sube la versión en `package.json`, `Cargo.toml` y `tauri.conf.json`, hace commit,
crea el tag `alpha-0.0.3` y lo sube. El tag lanza `.github/workflows/release.yml`, que comprueba
que tag y versión coinciden, compila y firma Windows y Linux y publica el Release con
`latest.json`. Las versiones usan tres números (`mayor.menor.parche`). Usa `--channel beta` o
`--channel v` para otros prefijos de tag.

El icono se genera desde `src-tauri/icons/source/app-icon.svg` con `npx tauri icon <png de 1024 px>`.

## Arquitectura

```
crates/
  bp-core/    Dominio puro, sin E/S ni UI
    scene       Escena fuente: mallas en mm, UV, texturas, color de vértice
    field       Color en cualquier punto (UV directa o transferencia por punto más cercano)
    palette     Paleta automática: k-means++ ponderado en Lab
    paint       Árbol de pintura 1→4 por triángulo (TriangleSelector de Bambu)
    encode      paint_color, bit a bit como Bambu Studio
    mesh        Reparación, piezas sueltas, simplificación (meshoptimizer), aplanado con corte real
    optimize    Limpieza de islas de color por adyacencia
    estimate    Cambios de filamento por capa y purga
    split       Una pieza cerrada por color (cáscara o sólido completo)
    bvh         Picking del pincel, consulta por esfera, punto más cercano
    project     Casos de uso: paleta, tamaño, operaciones, pincel, deshacer, vista previa
  bp-blend/   Infraestructura: ejecuta Blender headless con extract.py y lee el volcado binario
  bp-3mf/     Escritor del 3MF de Bambu, STL binario y ZIP por color
src-tauri/    Capa de aplicación: comandos, estado, malla en binario hacia la UI
src/          Interfaz (TypeScript + three.js), diseño Dose
  api.ts        Tipos y llamadas al backend
  viewer.ts     Visor, controles orbitales, pincel
  main.ts       Estados, arrastrar y soltar, ajustes, paneles
```

Blender solo extrae (evalúa modificadores y nodos de material); todo el cálculo
ocurre en Rust en paralelo (rayon). Con el modelo de ejemplo (326 756 triángulos,
9 texturas) la paleta, la pintura y la estimación tardan unos 170 ms; la extracción
con Blender unos 2,5 s.

### Por qué cada optimización acelera la impresión

- **Cambios de filamento**: cada capa con un color extra obliga a purgar y recargar.
  Limpiar islas y bajar el detalle de bordes quita colores sueltos de capas enteras.
  La estimación asume 0,2 mm por capa y 280 mm³ de purga por cambio; sirve para
  comparar ajustes, no como cifra exacta de Bambu Studio.
- **Simplificar**: menos triángulos laminan más rápido y el 3MF pesa menos; los colores
  se transfieren desde la malla original.
- **Reducir colores**: es la palanca más fuerte. Con el modelo de ejemplo, pasar de 10 colores
  detectados a 5 bajó los cambios estimados de 1 472 a 641 y la purga de 511 g a 223 g.
- **Aplanar base**: corta la base en un plano (la malla sigue cerrada) para dar más
  contacto con la placa y evitar soportes o despegues.
