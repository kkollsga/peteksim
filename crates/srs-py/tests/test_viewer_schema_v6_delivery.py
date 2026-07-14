"""Delivery acceptance for additive schema-v6 frame orientation.

The compute/forwarding identity is locked by the opt-in Rust exact-SHA test in
``viewer.rs``.  These tests lock the other half of petekSim's responsibility:
its save/server glue must hand a map and section to petekTools without rewriting
``rotation_deg``/``yflip`` or their schema version.  The fixture is synthetic and
starts from petekSim's real analytic-model payload so every unrelated required
bundle remains production-shaped.
"""

from __future__ import annotations

import json
import re
import threading
import urllib.parse
import urllib.request
from copy import deepcopy

import peteksim as ps


def _box():
    return ps.run_box_model(
        area_km2=0.04,
        gross_height_m=12.0,
        porosity=0.22,
        net_to_gross=0.71,
        water_saturation=0.31,
        fvf=1.25,
        fluid="oil",
        contact_m=2_009.0,
        top_m=2_000.0,
        ni=4,
        nj=4,
        nk=3,
    )


def _payload(model, tmp_path):
    target = tmp_path / "payload.json"
    model.save_json(str(target))
    payload = json.loads(target.read_text())
    frame = payload["map"]["frame"]
    line = [
        [frame["origin_x"], frame["origin_y"]],
        [
            frame["origin_x"] + (frame["ncol"] - 1) * frame["spacing_x"],
            frame["origin_y"] + (frame["nrow"] - 1) * frame["spacing_y"],
        ],
    ]
    section = json.loads(
        model._section_json(property="PORO", line=line, well=None)
    )
    payload["sections"] = [section]
    payload["section_labels"] = ["Synthetic diagonal"]
    return payload, line


def _oriented(payload):
    payload = deepcopy(payload)
    frame = payload["map"]["frame"]
    frame["rotation_deg"] = 30.0
    frame["yflip"] = True
    payload["map"]["schema_version"] = 6
    section = payload["sections"][0]
    section["schema_version"] = 6
    section["frame"] = deepcopy(frame)
    return payload


def _inlined_payload(html):
    match = re.search(
        r"window\.PETEK_VIEWER_PAYLOAD=(.*?);window\.PETEK_VIEWER_MODE=\"file\";",
        html,
        re.DOTALL,
    )
    assert match, "self-contained export did not inline its payload"
    return json.loads(match.group(1))


class _ForwardingModel:
    """The exact callback surface petekSim passes to petekTools' live server."""

    def __init__(self, payload):
        self.payload = payload

    def _section_json(self, **_request):
        return json.dumps(self.payload["sections"][0], separators=(",", ":"))

    def _volume_json(self, **_request):
        return json.dumps(self.payload["volume"], separators=(",", ":"))


def test_schema_v6_frame_is_byte_stable_through_save_and_serve(tmp_path):
    payload, line = _payload(_box(), tmp_path)
    payload = _oriented(payload)
    encoded = json.dumps(payload, separators=(",", ":"))

    html_path = tmp_path / "oriented.html"
    ps._save_view(str(html_path), encoded)
    inlined = _inlined_payload(html_path.read_text())
    assert inlined["map"] == payload["map"]
    assert inlined["sections"][0] == payload["sections"][0]
    assert inlined["map"]["frame"]["rotation_deg"] == 30.0
    assert inlined["map"]["frame"]["yflip"] is True

    httpd, url = ps._make_server(_ForwardingModel(payload), encoded, 0)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    try:
        with urllib.request.urlopen(url + "/model.json", timeout=5) as response:
            served = json.loads(response.read())
        assert served["map"] == payload["map"]
        assert served["sections"][0] == payload["sections"][0]

        query = urllib.parse.urlencode(
            {"line": json.dumps(line), "property": "PORO"}
        )
        with urllib.request.urlopen(url + "/section?" + query, timeout=5) as response:
            recut = json.loads(response.read())
        assert recut == payload["sections"][0]
        assert recut["frame"] == served["map"]["frame"]
    finally:
        httpd.shutdown()
        httpd.server_close()


def test_zero_and_pre_v6_payloads_remain_accepted(tmp_path):
    payload, _line = _payload(_box(), tmp_path)
    # A zero/default frame keeps the historical serialized member shape even
    # when its producer has moved to schema v6.
    assert "rotation_deg" not in payload["map"]["frame"]
    assert "yflip" not in payload["map"]["frame"]

    legacy = deepcopy(payload)
    legacy["map"]["schema_version"] = min(5, legacy["map"]["schema_version"])
    legacy_section = legacy["sections"][0]
    legacy_section["schema_version"] = min(5, legacy_section["schema_version"])
    legacy_section.pop("frame", None)
    target = tmp_path / "legacy.html"
    ps._save_view(str(target), json.dumps(legacy, separators=(",", ":")))
    inlined = _inlined_payload(target.read_text())
    assert inlined["map"] == legacy["map"]
    assert inlined["sections"][0] == legacy_section
