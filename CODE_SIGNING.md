# Code signing policy / Política de firma de código

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

Firma de código gratuita de [SignPath.io](https://about.signpath.io), certificado de [SignPath Foundation](https://signpath.org).

## What is signed / Qué se firma

The Windows installers of BlendParser published in [Releases](https://github.com/ArnulfoVargas/blend-parser-app/releases):
`BlendParser_<version>_x64-setup.exe` and `BlendParser_<version>_x64_<lang>.msi`.

They are built only by the [release workflow](.github/workflows/release.yml) on GitHub-hosted runners, from the
tagged source in this repository, and every signing request is approved by hand in SignPath. Nothing built on a
personal machine is signed.

Los instaladores de Windows se compilan solo en GitHub Actions a partir del código etiquetado de este repositorio,
y cada solicitud de firma se aprueba a mano en SignPath.

## Team roles / Roles del equipo

- Committers and reviewers / Autores y revisores: [ArnulfoVargas](https://github.com/ArnulfoVargas)
- Approvers / Aprobadores: [ArnulfoVargas](https://github.com/ArnulfoVargas)

All team members use multi-factor authentication on GitHub and SignPath.

## Privacy policy / Privacidad

This program will not transfer any information to other networked systems unless specifically requested by the
user or the person installing or operating it, with one exception: to look for updates, it downloads
`latest.json` from this repository's GitHub Releases when it starts and when the user clicks the version
number. That request sends nothing about the user or their files beyond what any HTTPS request to GitHub carries
(see [GitHub's privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement)).
An update is downloaded and installed only after the user accepts it.

The `.blend` files, the 3MF files and the printer profiles stay on the user's computer. BlendParser runs the
user's own Blender installation locally to read `.blend` files.

BlendParser no envía información a otros sistemas salvo para buscar actualizaciones: al abrirse, y cuando se pulsa
el número de versión, descarga `latest.json` de los Releases de GitHub de este repositorio. Una actualización solo
se descarga e instala si el usuario la acepta. Los archivos `.blend`, los 3MF y los perfiles de impresora no salen
del equipo.
