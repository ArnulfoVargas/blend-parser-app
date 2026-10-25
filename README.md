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
**Blender 3.6 o posterior** instalado. Los binarios no están firmados: en Windows,
SmartScreen puede pedir "Más información > Ejecutar de todas formas".

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

## Publicar una versión

Subir un tag `alpha-*`, `beta-*` o `v*` lanza `.github/workflows/release.yml`, que compila
Windows y Linux en GitHub Actions y publica un Release con los instaladores
(`alpha-` y `beta-` salen marcados como prerelease):

```sh
git tag alpha-0.0.1
git push origin alpha-0.0.1
```

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
