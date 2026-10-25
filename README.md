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
- Impresora y boquilla, con buscador: las 87 impresoras de una sola boquilla que trae Bambu
  Studio (Bambu Lab, Anker, Anycubic, Creality, Elegoo, Geeetech, Prusa, Qidi, Tronxy, Troodon,
  Voron y Voxelab), con las boquillas de su perfil, o una impresora propia con su volumen, sus
  boquillas y, si se quiere, el perfil de una del catálogo. Las propias y la última elección se
  guardan en `printers.json`, en la carpeta de configuración de la app.
- Tamaño por ejes (X, Y, Z) con bloqueo de proporciones, escala en %, tamaño original y
  deshacer; ajuste a la placa que respeta el margen, las zonas excluidas de la impresora y
  la reserva de la torre de purga. Regla para medir entre dos puntos del modelo (`M`).
- Desperdicio estimado: purga por cambio según el sentido del cambio de color (editable por
  par y exportada al 3MF), multiplicador, purga inicial, torre y purga aprovechada en relleno,
  con desglose en g, m y mm³. Los ajustes se guardan en `settings.json`.
- Interfaz en español e inglés (menú Editar > Preferencias).
- La boquilla manda en la preparación: los bordes de color no se cortan más fino que media
  boquilla, la estimación usa la altura de capa de su perfil y se avisa de las manchas de color
  demasiado pequeñas para ella.
- Optimizaciones para imprimir más rápido: limpiar islas de color, nivel de detalle de bordes,
  simplificar la malla y aplanar la base, con estimación de cambios de filamento y purga.
- Reparación de malla: une vértices duplicados, quita triángulos degenerados y caras repetidas,
  y elimina piezas sueltas menores que un tamaño dado.
- Exportar por color: un `.zip` con un STL cerrado por color (cáscara del grosor elegido hacia
  dentro; opcionalmente el color principal como sólido completo) y una lista de colores.

## Perfiles de impresora en el 3MF

El catálogo es `crates/bp-3mf/src/catalog.json`, generado desde `resources/profiles` de
[BambuStudio](https://github.com/bambulab/BambuStudio) con:

```sh
node scripts/sync-printers.mjs [commit]
```

La build de Cargo convierte el JSON versionado en una tabla Rust dentro de `OUT_DIR`.
La tabla queda embebida en el ejecutable: la app no consulta GitHub ni parsea el JSON
al arrancar. El bake es offline, se repite al cambiar el JSON o el generador y valida
identificadores, volumen, boquillas y presets antes de compilar. El script de arriba
solo se ejecuta manualmente para actualizar los datos desde Bambu Studio; no forma
parte de la build. Guarda el JSON actualizado en Git para reproducir el mismo bake.

Para cada impresora y boquilla, el 3MF nombra el preset de máquina de Bambu Studio, su proceso
por defecto (o el compatible con la capa más cercana a media boquilla) y un PLA para ella. Una
impresora propia sin perfil lleva en el 3MF su volumen (`printable_area`, `printable_height`),
la boquilla y la altura de capa.

Quedan fuera las impresoras de dos extrusores (H2D, H2D Pro, H2C, X2D): su 3MF necesita otra
configuración de filamentos.

## Torre de purga y desperdicio

BlendParser **no cambia la torre** que el 3MF pide a Bambu Studio: sigue en la posición de
siempre (`wipe_tower_x/y`) y la genera el laminador. La app estima su huella (20 mm de ancho
más 3 mm de borde, y un fondo según la purga de la capa con más cambios, o el que se indique)
y aparta ese espacio, con la separación y el margen elegidos, al comprobar si el modelo cabe
y al ajustarlo a la placa. El modelo sigue centrado. Las zonas excluidas (`bed_exclude_area`)
salen del perfil de Bambu Studio.

Los cambios se cuentan como el laminador los hace: un color solo cuenta en una capa si ocupa
al menos dos líneas de la boquilla (las astillas de borde no provocan un cambio), y el orden de
los colores se planifica en todas las capas para purgar lo mínimo: cada capa empieza con el
filamento cargado y termina en el que conviene a la siguiente.

El desperdicio suma tres partes, cada una una sola vez: purga en los cambios (a la basura;
la parte aprovechada en relleno o soportes se muestra aparte y no suma), purga inicial y
torre (paredes, relleno de capas sin cambios, borde y cebado por cambio). En las impresoras
sin conducto de purga la purga acaba en la torre, pero se cuenta como purga. La purga por
cambio va de 107 a 800 mm³ según ΔE y lo que aclara el nuevo color, como sugiere Bambu Studio.

Pendiente: comparar la estimación con lo que informa Bambu Studio al laminar. La CLI de
Bambu Studio 2.x se cierra al laminar desde la línea de comandos en Windows, así que hay que
hacerlo desde la interfaz: exportar el modelo de ejemplo a 75 mm con 4 colores y anotar la
diferencia con «Filamento → Purgado» y «Torre» del resumen de laminado. Con ese modelo la app
estima 430 cambios, 164,4 g de purga, 15,0 g de torre y 179,5 g en total.

## Idiomas

Los textos de la interfaz están en `src/i18n/`: `es.ts` es la referencia y el respaldo, y cada
idioma es un archivo con las mismas claves. En el primer arranque se usa el idioma del sistema
si hay catálogo para él; si no, el español. La elección se guarda en el almacenamiento de la
ventana. El backend no redacta mensajes: envía una clave (`error.*`, `progress.*`) con sus
valores y la interfaz la traduce.

Para añadir un idioma:

1. Copia `src/i18n/en.ts` como `xx.ts` y traduce los textos. Respeta los huecos `{nombre}`; las
   entradas `{ one, other }` son plurales.
2. Regístralo en `LANGS` y `CATALOGS` en `src/i18n/index.ts`, con su nombre en ese idioma.
3. Si el ZIP por color debe salir en ese idioma, añade su `ZipText` en `crates/bp-3mf/src/lib.rs`.
4. Comprueba: `npm run build` falla si al catálogo le falta o le sobra una clave, y
   `npm run check:i18n` revisa textos vacíos, huecos y las claves que usan `index.html`, el
   backend y `extract.py`. Las dos corren en CI.

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
    estimate    Cambios de filamento por capa, con su sentido
    waste       Desperdicio: purga por par de colores, purga inicial y torre
    split       Una pieza cerrada por color (cáscara o sólido completo)
    bvh         Picking del pincel, consulta por esfera, punto más cercano
    project     Casos de uso: paleta, tamaño, operaciones, pincel, deshacer, vista previa
  bp-blend/   Infraestructura: ejecuta Blender headless con extract.py y lee el volcado binario
  bp-3mf/     Escritor del 3MF de Bambu, STL binario y ZIP por color; plate: espacio en la placa
src-tauri/    Capa de aplicación: comandos, estado, malla en binario hacia la UI
src/          Interfaz (TypeScript + three.js), diseño Dose
  api.ts        Tipos y llamadas al backend
  i18n/         Catálogos de textos y formatos por idioma
  menubar.ts    Barra de menús
  viewer.ts     Visor, controles orbitales, pincel, regla
  main.ts       Estados, arrastrar y soltar, ajustes, paneles
```

Blender solo extrae (evalúa modificadores y nodos de material); todo el cálculo
ocurre en Rust en paralelo (rayon). Con el modelo de ejemplo (326 756 triángulos,
9 texturas) la paleta, la pintura y la estimación tardan unos 170 ms; la extracción
con Blender unos 2,5 s.

### Por qué cada optimización acelera la impresión

- **Cambios de filamento**: cada capa con un color extra obliga a purgar y recargar.
  Limpiar islas y bajar el detalle de bordes quita colores sueltos de capas enteras.
  La estimación usa la altura de capa del perfil y la purga de cada par de colores; sirve
  para comparar ajustes, no como cifra exacta de Bambu Studio.
- **Simplificar**: menos triángulos laminan más rápido y el 3MF pesa menos; los colores
  se transfieren desde la malla original.
- **Reducir colores**: es la palanca más fuerte. Con el modelo de ejemplo, pasar de 10 colores
  detectados a 5 bajó los cambios estimados de 1 472 a 641.
- **Aplanar base**: corta la base en un plano (la malla sigue cerrada) para dar más
  contacto con la placa y evitar soportes o despegues.

### Diagnóstico local de viabilidad FDM (alcance inicial de #7)

En **Viabilidad de impresión**, pulsa **Analizar modelo** después de elegir el tamaño final,
la impresora, boquilla y altura de capa. El análisis usa la geometría de trabajo actual,
con su escala por eje, simplificación y base aplanada; no la escena sin transformar.
El informe registra dimensiones, impresora, boquilla, capa y todos los umbrales usados.
No hay una puntuación de éxito: cada comprobación distingue **comprobado**, **estimado**
y **no evaluado**. Cualquier cambio de documento, geometría, tamaño, perfil o umbral
invalida el informe; hay que ejecutarlo de nuevo. El idioma no lo invalida.

- **Comprobado:** bordes abiertos (no un conteo de agujeros), aristas con más de dos caras,
  continuidad del abanico de caras de cada vértice, caras duplicadas y degeneradas,
  orientación entre caras, signo del volumen por componente cerrado y límites de la
  impresora con margen y reserva de torre del panel Placa. La topología se comprueba
  por objeto después de soldar posiciones mediante una cuadrícula de tolerancia en mm;
  posiciones muy próximas a lados distintos de una celda pueden seguir separadas.
- **Estimado:** componentes separados o pequeños, detalle fino por la menor dimensión de
  cada componente, componentes que no tocan la placa ni otra pieza apoyada (por caja
  envolvente), voladizos y área de contacto.
  Las piezas separadas pueden ser intencionales; nunca se eliminan automáticamente por
  un hallazgo. Convención angular: **0° = pared vertical; 90° = cara inferior horizontal**.
  El contacto es el área de caras inferiores dentro de la tolerancia del mínimo Z.
- **Espesor muestreado:** rayos desde centroides hacia dentro de la malla, en mm finales;
  sólo se calcula con superficies cerradas, orientadas y sin los defectos topológicos
  detectados. El umbral efectivo es el mayor entre pared mínima y ancho de extrusión
  por perímetros objetivo. El mapa usa rojo para muestras finas, cian para muestras sobre
  el umbral y gris cuando el rayo no encuentra una pared. No constituye un mapa continuo:
  puede omitir paredes o detalles locales, acertar otra envolvente o ser engañoso ante
  intersecciones. Ausencia de muestras finas no significa aprobación de toda la superficie.

Los valores iniciales son editables y restaurables: ancho de extrusión = 1,1 veces la
boquilla, pared = dos anchos, ángulo = 45°, contacto = 4 mm² y componente = 2 mm de
**diagonal de caja**. Son puntos de partida para calibración FDM, no límites universales
ni umbrales de un proveedor externo. La tolerancia admite 0,00001–0,1 mm, la calidad
16–4096 muestras, el ancho 0,05–3 mm, pared/detalle 0,05–20 mm y 1–20 perímetros.
La tecnología inicial es FDM; el análisis no modela un material ni los ajustes del laminador.

Cada hallazgo ofrece **Localizar** con hasta 32 marcadores sobre el modelo. Las mediciones
usan las unidades indicadas en el título: aristas/caras/componentes, mm, mm² o grados.
En modelos grandes, las comprobaciones topológicas recorren vértices y caras y el BVH
cuesta aproximadamente O(T log T); los rayos se limitan por la calidad elegida. No hay
comparación de todos los pares de triángulos. Una referencia de 24.000 triángulos en los
tests comprueba que las muestras y marcadores siguen acotados; no es un benchmark de
rendimiento ni una garantía de memoria para modelos arbitrarios. El análisis se ejecuta
bajo demanda en un trabajador del backend; cada corrección conserva una copia
completa del proyecto y puede consumir memoria adicional en modelos grandes.

**Corregir** aparece junto a **Localizar** en los hallazgos que el núcleo sabe arreglar.
Cada corrección activa operaciones de malla deterministas y vuelve a analizar:

| Hallazgo | Corrección |
| --- | --- |
| Caras degeneradas o duplicadas, aristas no manifold | Limpieza a fondo: soldar vértices a 0,001 mm, eliminar caras degeneradas y duplicadas y quitar caras sueltas que cuelgan de una sola arista (grupos de hasta una décima parte de la superficie principal) |
| Orientación inconsistente, volumen invertido | Propagar la orientación entre caras vecinas y dar la vuelta a envolventes con volumen negativo |
| Bordes abiertos | Limpieza a fondo y cerrar cada agujero con un abanico plano desde su centro, del color de la cara vecina |
| Piezas pequeñas | Eliminar piezas sueltas bajo el umbral |
| Poco contacto con la placa | Aplanar la base (dos capas, mínimo 0,4 mm) |
| Vértices no manifold | Limpieza a fondo y dar a cada grupo de caras su propia copia del vértice, a 0,02 mm |
| Piezas sin contacto con la placa | Bajar hasta la placa las piezas que no se apoyan en nada |
| Caras en voladizo | Probar unas cincuenta orientaciones (ejes, aristas, esquinas y las caras más grandes) y girar a la de menor área en voladizo entre las que se sostienen solas: el centro de masa debe caer dentro de la huella de apoyo |
| No cabe en la placa | Reducir el tamaño sin deformar hasta que quepa con margen y torre |

Girar y ajustar el tamaño conservan la pintura manual; un giro descarta el estiramiento
por ejes que hubiera, porque los ejes cambian, y **Revertir** lo devuelve. Las piezas
separadas se informan sin aviso: son normales en modelos de varios objetos. Bajar piezas mueve solo la malla final: los
colores se siguen leyendo donde estaban.

Una vez aplicada, la tarjeta queda como **Corregido** con el conteo antes/ahora y el botón
pasa a **Revertir**, que devuelve esas operaciones a sus valores previos. Cada corrección o
reversión es un paso de **Deshacer** que restaura geometría, ajustes, materiales, paleta y
pintura. Si hay pintura manual, se pide confirmación, porque rehacer la malla la retira.
Las tapas de agujeros grandes o curvos son planas: revísalas antes de exportar.

**Pendiente / no evaluado:** autointersecciones robustas (incluidas caras coplanares),
puentes, inicios locales de capas sin apoyo, distancias entre piezas y fragilidad mecánica.
No se ofrecen engrosado local ni remallado; la orientación automática solo prueba un
conjunto fijo de direcciones.
El cierre de agujeros no reconstruye superficies curvas. El diagnóstico tampoco mide la desviación
respecto a la geometría anterior a simplificar: vuelve a revisar la geometría resultante,
pero no certifica que todos los detalles originales sobrevivan.

Pruebas de referencia cubren caja cerrada, agujero, cara invertida, volumen invertido,
caras degeneradas/duplicadas, aristas no manifold, vértice que une dos envolventes,
pared fina, componente pequeño/aislado, geometría elevada sin contacto y dos cajas
intersectadas. Este último caso comprueba expresamente que **autointersecciones** aparece
como **no evaluado**, nunca como aprobado. También se verifica la coherencia al cambiar
escala/boquilla/umbrales y la restauración del color al deshacer una corrección.

La comparación antes/después en Bambu Studio u otro laminador **no está automatizada ni
verificada en esta entrega**: exporta ambos 3MF, selecciona el mismo proceso/material y
compara continuidad de capas, detalles, soportes y adhesión en la vista laminada antes de
imprimir. La ausencia de hallazgos no garantiza que la pieza se imprima sin defectos.

El volumen y espesor de modelos con envolventes de signos mixtos quedan **no evaluados**:
una envolvente interior invertida puede ser una cavidad correcta y este alcance no
implementa una prueba robusta de anidamiento. Una envolvente aislada invertida sigue
siendo un error. Deshacer una corrección conserva la altura de capa activa; cambiar la
boquilla reconstruye la pintura e invalida ese historial, como los otros cambios de malla.
