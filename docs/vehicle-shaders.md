# Vehicle shaders

Which shader each part of a vehicle should use. Relevant to the tint problem (see [vehicle-flags-and-glass.md](vehicle-flags-and-glass.md)).

Source: <https://www.modding-forum.com/guide/14-materials-and-shaders/> (community guide, written for ZModeler 3; not an official reference, and the official Cfx docs have no page for shaders). Checked on 2026-11-12.

## Shaders in common use

| Shader | Purpose | Textures |
|---|---|---|
| `vehicle_mesh` | Opaque metal or plastic. Exterior parts in the default game; many modders also use it for interiors | Diffuse, Normal, Dirt (UV 2), Specular, Environment |
| `vehicle_vehglass` | Any transparent glass, **exterior-facing** layer. Supports transparency | Diffuse, Dirt (UV 2), Specular, Environment |
| `vehicle_vehglass_inner` | Same, **interior-facing** layer. A window has two layers | Diffuse, Dirt (UV 2), Specular, Environment |
| `vehicle_lightsemissive` | Transparent or opaque emissive parts: head and tail lights | Diffuse, Dirt (UV 2), Specular, Environment |
| `vehicle_tire` | Wheels. The whole wheel mesh must use it | Diffuse, Normal, Dirt, Specular, Environment |
| `vehicle_licenseplate` | Plates; switches texture and text in game | Diffuse, Normal, Dirt, Environment |
| `vehicle_badges` | Badges. Also used as an overlay over materials without normal maps, such as vehglass | Diffuse, Normal, Burnt, Specular, Environment |
| `vehicle_interior`, `vehicle_interior2` | Interior parts in the default game | Diffuse, Normal or Burnt, Specular |
| `vehicle_paint1` | Opaque paint that takes the in-game color. A transparent diffuse overlays a texture | Diffuse, Dirt (UV 2), Specular, Environment |
| `vehicle_paint3` | Same, with livery support: first diffuse is the white `vehicle_generic_smallspecmap`, second is the livery; names end in `sign_1` to `sign_25` | Diffuse 1, Diffuse 2 (UV 2), Dirt, Specular, Environment |
| `vehicle_shuts` | Paint over an opaque interior body texture. Always used with paint | Diffuse, Normal, Burnt, Specular |

Notes from the guide:

- Texture channel is UV 1 unless stated otherwise. Dirt maps use UV 2. Environment maps are "Auto Sphere" and have no UV.
- In-game paint colors: the material name ends in `[PAINT:X]` (usually 1 for primary, 2 for secondary). The color sets come from `carvariations.meta`.
- `vehicle_paint1`, `vehicle_paint3` and `vehicle_shuts` should always be used with paint.

## What this suggests for the tinted lights (hypothesis, not tested)

The in-game color and tint apply by shader. A light that ends up tinted is probably using a paint shader, or a `[PAINT:X]` material name, or `vehicle_vehglass` instead of `vehicle_lightsemissive`; or it shares UVs with a painted region. Checking is cheap: list the shader of every geometry in the `.yft` and flag lens/light geometry that is not `vehicle_lightsemissive`. Needs a vehicle where it happens to confirm.

## Sources reviewed and not used

- <https://gtamods.com/wiki/Material_List_(RW_Section)>: RenderWare (GTA III, Vice City, San Andreas) material list inside a DFF. A different engine and format from GTA V's RAGE; nothing here applies.
- <https://gtaforums.com/topic/899267-entities-and-materials/>: not read. The site answered with a Cloudflare challenge (HTTP 403).
