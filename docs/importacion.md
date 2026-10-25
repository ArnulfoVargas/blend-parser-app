# Importación multicolor

La selección, el arrastre y los argumentos de línea de comandos admiten BLEND, OBJ,
FBX, glTF/GLB y PLY (extensiones sin distinción de mayúsculas). Todos utilizan el
Blender instalado, en modo local y sin ejecutar scripts del archivo. No se necesita
un servicio externo ni complementos descargados. Si la distribución de Blender omite
un importador, la aplicación muestra el error del importador y conserva el documento
anterior. Se recomienda Blender oficial 3.6 o posterior; las pruebas reales de esta
implementación se ejecutaron con Blender 5.0.1 en Windows.

| Formato | Color conservado | Recursos y unidades |
| --- | --- | --- |
| BLEND | Material por cara, color base, imagen UV o atributo de color conectado | Modificadores evaluados, transformaciones y unidades de escena. Sin unidades: 1 unidad = 1 mm. |
| OBJ | Piezas, material por cara, `Kd` de MTL, textura difusa UV | Mantén OBJ, MTL e imágenes en sus rutas relativas originales. 1 unidad = 1 mm. |
| FBX | Piezas, transformaciones, materiales por cara, color base, textura UV y atributos conectados | FBX binario compatible con el importador de Blender; FBX ASCII no está admitido. Texturas relativas o embebidas. La escena importada está en metros y se convierte a mm. |
| glTF / GLB | Piezas, material por cara, color base, textura UV y atributos conectados, incluidos materiales `KHR_materials_unlit` | GLB puede incluir imágenes y geometría. glTF requiere sus `.bin` e imágenes externos. Metros → mm. |
| PLY | Geometría y atributo activo de color por vértice | Solo mallas con caras; una nube de puntos no genera superficie imprimible. 1 unidad = 1 mm. Sin color: gris con aviso. |

Los valores OBJ y PLY no declaran unidad física: el panel muestra un aviso de unidades;
verifica el tamaño final antes de exportar. La orientación usa las convenciones del
importador de Blender (OBJ: Y arriba, frente −Z; PLY: Z arriba). La aplicación coloca
el modelo sobre Z = 0 y centra XY como ya hace con BLEND.

La asignación por cara y las piezas pasan al mismo flujo de paleta, pintura,
separación por color y exportación 3MF. Los colores por vértice se interpolan por
coordenadas baricéntricas. Las imágenes se muestrean en UV con repetición; la paleta
discretiza esos colores a los filamentos elegidos y subdivide sus bordes.

Los avisos permanecen en Materiales después de cambiar tamaño, paleta o impresora:

- MTL o textura ausentes/ilegibles: la importación utiliza el color disponible o gris.
  Recupera el recurso en su ruta original y abre el modelo otra vez.
- Imagen sin UV: se sustituye por su color medio y se indica el material afectado.
- Mezclas, mapas transformados, proyecciones y modos de extensión no repetidos o
  shaders procedurales: se usa una fuente base y se avisa que es aproximada.
- Modelo sin material ni atributo activo: gris, con el nombre de la malla afectada.
- glTF sin su geometría `.bin`, FBX ASCII, archivo corrupto o importador ausente:
  error de importación con el detalle de Blender, sin sustituir el documento abierto.

No se reproducen iluminación, transparencia, rugosidad, metal, normales ni apariencia
PBR: el destino son colores opacos de filamento. Las imágenes se aproximan a un único
color base por muestra; factores y mezclas del shader pueden requerir hornear primero
una textura de color. La animación no se importa como una secuencia; se extrae el estado
evaluado del modelo. NURBS y nubes de puntos sin caras no se convierten automáticamente.

## Archivos que acompañan al modelo

Antes de convertir un OBJ, glTF o FBX, la app lee el modelo (sin abrir Blender) y busca lo
que referencia: el `.mtl` del OBJ y sus texturas, el `.bin` y las imágenes del glTF, y las
texturas externas del FBX (las incrustadas no hacen falta). Una referencia se encuentra
tal como está escrita junto al archivo que la nombra, junto al modelo, por nombre junto
al modelo o entre los archivos que agregue el usuario (sin distinguir mayúsculas).

Si falta algo, el diálogo **Archivos del modelo** muestra un paso por tipo: Materiales,
Geometría y Texturas. Todos son opcionales salvo el `.bin`, sin el cual no hay malla.
Agregar un `.mtl` vuelve a leerlo y puede descubrir las texturas que pide. En Texturas se
puede elegir una carpeta (cuenta su contenido y el de sus subcarpetas directas). Soltar el
modelo junto con sus archivos también los usa. Si no falta nada, no aparece el diálogo.

Al importar, los archivos agregados se pasan al extractor. Si el `.mtl` o el `.bin` vienen
de otra carpeta, se importa una copia del modelo desde una carpeta temporal que los tiene
con el nombre que el modelo espera; las imágenes que no cargaron se vuelven a enlazar por
nombre. `companions.rs` y `extract.py` siguen las mismas reglas.

## Otros formatos evaluados

| Formato | Prioridad | Decisión / limitación |
| --- | --- | --- |
| glTF/GLB | Alta | Admitido; GLB facilita mover el modelo sin perder recursos externos. |
| PLY con color | Alta | Admitido; atributo activo interpolado por cara. |
| 3MF | Siguiente | No admitido como entrada. El color puede estar en recursos estándar o en pintura jerárquica privada de Bambu; se necesita un lector que conserve ambos y sus transformaciones. Renombrarlo o importar solo su malla descartaría color. |
| STL | Baja | No admitido: no tiene una representación estándar de materiales o color que satisfaga este flujo. |
| USD / Alembic | Baja | No admitidos: requieren definir variantes, instancias, tiempo y compatibilidad de materiales antes de prometer conservación de color. |

## Validación reproducible

```sh
cargo test -p bp-blend --test multiformat
cargo test -p bp-blend --test multiformat -- --ignored --nocapture
```

El segundo comando exige Blender y genera localmente dos cubos (24 triángulos,
40 × 10 × 10 mm), material rojo, textura azul y una variante PLY con color por
vértice. Comprueba los seis formatos, sus dimensiones, muestras rojas y azules,
texturas y exportación 3MF; verifica material por cara en ambos objetos, materiales
glTF sin iluminación y avisos de MTL e imagen ausentes con espacios o tabuladores.
Los fixtures FBX/GLB se generan en vez de almacenar binarios opacos en Git.

Referencias del importador: [operadores OBJ/PLY](https://docs.blender.org/api/current/bpy.ops.wm.html)
y [operadores FBX/glTF](https://docs.blender.org/api/current/bpy.ops.import_scene.html).
