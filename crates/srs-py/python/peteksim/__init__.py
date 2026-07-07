"""petekSim — fast field/discovery appraisal toolkit (Rust core, Python API).

Code-first: build a model in Python, then ``model.view()`` pops open a three.js
render of it in your browser. The compute lives in the Rust extension
(``peteksim._core``); the renderer is petekTools' horizontal ``petektools.viewer``
unit (owner ruling), which this package feeds a render payload and a live-section
callback.

    import peteksim
    m = peteksim.run_box_model(area_km2=(0.32, 0.4, 0.52), gross_height_m=(12, 15, 20),
                                 porosity=0.25, net_to_gross=0.8, water_saturation=0.3,
                                 fvf=1.25, fluid="oil", contact_m=2743)
    m.view()                       # opens the 3D viewer in a browser

    sm = peteksim.Model(0.4, 15.0, ni=24, nj=24, nk=8,
                          top_m=1500, contact_m=1510.5)
    sm.add_control(12, 12, 1489)   # a structural high, in code
    sm.view()                      # build + view

All inputs are SI/metric (area km², lengths/depths m positive-down, FVF
Rm³/Sm³); results in Sm³ (GRV in mcm = 10⁶ m³).
"""

from ._core import (
    Model,
    ModelResult,
    Refined,
    run_box_model,
    version,
    # --- staged model-build facade (Project.load -> ... -> model.uncertainty) ---
    Project as _CoreProject,
    Inventory,
    Wells,
    Surface,
    Tops,
    TopsPick,
    Framework,
    Grid as _CoreGrid,
    Property,
    StaticModel,
    Uncertainty,
    ZonedUncertainty,
    Dist,
    PickSpread,
    Vgm,
    GaussianSpec,
    Trend,
    Layers,
    normal,
    lognormal,
    uniform,
    triangular,
    truncated_normal,
    level_shift,
    pick_spread,
    spherical,
    exponential,
    gaussian_vgm,
    fit_variogram,
    gaussian,
    resimulate,
    layers,
    aggregate,
    distribution_bundle,
)

# --- modelling API v2 — the declarative spec surface + application driver -----
# The v2 layer is a thin Python facade over the Rust `_core` engine: specs are
# immutable values (to_dict/from_dict/replace/eq/table-repr, conformance battery
# R7); applications are explicit moments. `Project`/`collocated` below OVERRIDE
# the `_core` bindings with the v2 wrappers (the v1 chain keeps working, with a
# DeprecationWarning, through the wrapped Project).
from .apply import Project, collocated  # noqa: E402
from .specs import (  # noqa: E402
    Spec, spec_from_dict, registered_specs, NotYetSupported, ApplyError,
    hz, splits, zone, Extrapolation, decay_to_flat, flat, nearest,
    variogram, shift, dist, Propagate, Variogram, CollocatedTrend,
    Horizons_factory as Horizons, Subzones_factory as Subzones,
    Layering_factory as Layering, Contacts_factory as Contacts,
    TieSettings_factory as TieSettings, Gridding_factory as Gridding,
    Run_factory as Run, LoadSettings_factory as LoadSettings,
    ViewSettings_factory as ViewSettings, Prop_factory as Prop,
    Props_factory as Props, Mc_factory as Mc, McSettings,
    crossplot as Crossplot, tornado as Tornado, distribution as Distribution,
    AssetSpec,
)

from .synth_asset import synth_asset  # noqa: E402  (the complete synthetic-asset composer)

_PETEKSTATIC_EXPORTS = (
    "CoKriging",
    "DistributionSpec",
    "Grid",
    "PropertyHandle",
    "PropertyPipeline",
    "PropertyPipelineSpec",
    "PropertyStore",
    "SgsRecipe",
    "Spherical",
    "UpscaleRecipeBuilder",
    "Var",
    "VolumeCase",
    "VolumeResult",
    "WellLog",
    "WellLogSpec",
    "distributions",
    "upscale",
)


def _petekstatic():
    try:
        import petekstatic
    except ModuleNotFoundError as exc:
        if exc.name != "petekstatic":
            raise
        raise ImportError(
            "peteksim's static property-workflow compatibility shim delegates to "
            "petekstatic. Install petekstatic, or import petekstatic directly, to "
            "use ps.Grid/ps.upscale/ps.distributions/ps.Var and related canonical "
            "property workflow APIs."
        ) from exc
    return petekstatic


def __getattr__(name):
    if name in _PETEKSTATIC_EXPORTS:
        return getattr(_petekstatic(), name)
    raise AttributeError(f"module 'peteksim' has no attribute {name!r}")


def __dir__():
    return sorted(set(globals()) | set(_PETEKSTATIC_EXPORTS))


__all__ = [
    "synth_asset",
    "Model", "ModelResult", "Refined", "run_box_model", "version", "view",
    # facade (v1 + v2-wrapped Project)
    "Project", "Inventory", "Wells", "Surface", "Tops", "TopsPick",
    "Framework", "Property", "StaticModel", "Uncertainty", "ZonedUncertainty",
    "Dist", "PickSpread", "Vgm", "GaussianSpec", "Trend", "Layers",
    "normal", "lognormal", "uniform", "triangular", "truncated_normal",
    "level_shift", "pick_spread", "spherical", "exponential", "gaussian_vgm",
    "fit_variogram", "gaussian", "collocated", "resimulate", "layers", "aggregate",
    "distribution_bundle",
    # --- modelling API v2 ---
    "Spec", "spec_from_dict", "registered_specs", "NotYetSupported", "ApplyError",
    "Horizons", "hz", "Subzones", "splits", "Layering", "Contacts", "zone",
    "TieSettings", "Gridding", "Extrapolation", "decay_to_flat", "flat", "nearest",
    "Prop", "Props", "Propagate", "Variogram", "CollocatedTrend",
    "variogram", "Mc", "McSettings", "shift", "dist",
    "Run", "LoadSettings", "ViewSettings", "AssetSpec",
    "Crossplot", "Tornado", "Distribution",
    # canonical petekStatic property workflow, reached through petekSim as a shim
    "CoKriging", "DistributionSpec", "Grid", "PropertyHandle",
    "PropertyPipeline", "PropertyPipelineSpec", "PropertyStore", "SgsRecipe",
    "Spherical", "UpscaleRecipeBuilder", "Var", "VolumeCase", "VolumeResult",
    "WellLog", "WellLogSpec", "distributions", "upscale",
]


#   --- viewer glue: thin over the horizontal petektools.viewer unit ---------
# The bundle renderer (JS assets + serve/save_view) is petekTools' viewer unit
# (owner ruling decision_viewer_home_petektools, 2026-07-04): it serves all
# layers, so it is horizontal capability and lives downstream in the DAG. peteksim
# is now a *consumer* — model.view()/save_view()/the fence endpoint are thin glue
# over petektools.viewer; the render payload is peteksim's (composed in Rust from
# petekStatic's typed bundles), mapped onto petektools' generic render schema.
# These three functions keep their signatures (the Rust bindings call them).


def _section_provider(model):
    """The pluggable /section callback for the live server: answer a fence/well
    request by cutting a fresh section through ``model`` (peteksim's compute). This
    is the domain half petektools.viewer knows nothing about."""

    def provider(line=None, well=None, property=None):
        return model._section_json(property=property, line=line, well=well)

    return provider


def _volume_provider(model):
    """The pluggable /volume callback for the live server (mirrors
    ``_section_provider``): RE-CUT the exterior shell at a property ``cutoff`` so the
    interior faces the cut exposes are rendered server-side. Returns the v3 volume
    envelope JSON. Only wired in live/served mode — a ``save_view`` file ships the
    full-set shell + the client-side shell filter."""

    def provider(property=None, cutoff=None, keep_above=True):
        return model._volume_json(property=property, cutoff=cutoff, keep_above=keep_above)

    return provider


def _make_server(model, payload_json: str, port: int):
    """Build (but don't start) the local viewer server — delegates to
    ``petektools.viewer.build_server`` with model-backed ``/section`` + ``/volume``
    providers. Returns ``(httpd, url)``."""
    from petektools import viewer

    return viewer.build_server(
        payload_json,
        port=port,
        section_provider=_section_provider(model),
        volume_provider=_volume_provider(model),
    )


def _serve(
    model,
    payload_json: str,
    open_browser: bool = True,
    port: int = 0,
    block: bool = False,
) -> str:
    """Serve the viewer for ``model``; return the URL. Thin over
    ``petektools.viewer.serve`` — the live ``/section`` endpoint is routed back to
    ``model._section_json``. Called by the Rust ``view()`` methods."""
    from petektools import viewer

    return viewer.serve(
        payload_json,
        port=port,
        block=block,
        open_browser=open_browser,
        section_provider=_section_provider(model),
        volume_provider=_volume_provider(model),
    )


def _serve_charts(
    payload_json: str,
    open_browser: bool = True,
    port: int = 0,
    block: bool = False,
) -> str:
    """Serve a **charts-only** payload (a pure-analytics `mc.view()` session) — thin
    over ``petektools.viewer.serve`` with no ``/section`` provider (there is no model
    to cut sections through). Called by the Rust ``Uncertainty.view()``."""
    from petektools import viewer

    return viewer.serve(
        payload_json, port=port, block=block, open_browser=open_browser, section_provider=None
    )


def _save_view(path: str, payload_json: str) -> None:
    """Write ONE self-contained HTML file — thin over
    ``petektools.viewer.save_view`` (all JS + data inlined; zero external
    fetches)."""
    from petektools import viewer

    viewer.save_view(payload_json, path)


def view(model, open_browser: bool = True, port: int = 0, block: bool = False):
    """Open the viewer for any model/result (same as ``model.view()``)."""
    return model.view(open_browser, port, block)
