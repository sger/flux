#!/usr/bin/env python3
import json
import pathlib
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"

with open(ROOT / "package.json", "r", encoding="utf-8") as f:
    pkg = json.load(f)

publisher = pkg["publisher"]
name = pkg["name"]
version = pkg["version"]
engine = pkg.get("engines", {}).get("vscode", "*")
display_name = pkg.get("displayName", name)
description = pkg.get("description", "")
categories = ",".join(pkg.get("categories", []))
tags = ",".join(pkg.get("keywords", []))

extension_id = f"{publisher}.{name}"
vsix_name = f"{name}-{version}.vsix"
vsix_path = DIST / vsix_name

DIST.mkdir(parents=True, exist_ok=True)

manifest = f'''<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Id="{extension_id}" Version="{version}" Language="en-US" Publisher="{publisher}" />
    <DisplayName>{display_name}</DisplayName>
    <Description xml:space="preserve">{description}</Description>
    <Tags>{tags}</Tags>
    <Categories>{categories}</Categories>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="{engine}" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
  </Assets>
</PackageManifest>
'''

content_types = '''<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="json" ContentType="application/json" />
  <Default Extension="vsixmanifest" ContentType="text/xml" />
  <Default Extension="md" ContentType="text/markdown" />
  <Default Extension="txt" ContentType="text/plain" />
  <Default Extension="xml" ContentType="text/xml" />
</Types>
'''

include_files = [
    "package.json",
    "README.md",
    "language-configuration.json",
    ".vscodeignore",
    "syntaxes/flux.tmLanguage.json",
]

with zipfile.ZipFile(vsix_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
    zf.writestr("extension.vsixmanifest", manifest)
    zf.writestr("[Content_Types].xml", content_types)

    for rel in include_files:
        src = ROOT / rel
        if src.exists():
            zf.write(src, f"extension/{rel}")

print(vsix_path)
