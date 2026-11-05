"""SimLab's meshing helper. Gmsh turns a STEP or STL into:

  surface  the part's surface as triangles, each tagged with the surface
           patch it belongs to (a CAD face, or a region found by angle)
  volume   a quadratic tetrahedral (tet10) mesh for the stress test, with
           its boundary faces tagged by the same patches

Usage:
  python simlab_mesher.py surface <in.step|in.stl> <out_dir> [--angle 40]
  python simlab_mesher.py volume  <in.step|in.stl|surface_dir> <out_dir> --size <mm> [--unit 0.001] [--angle 40]
           (surface_dir: a surface in the `surface` output format, e.g. a
           decimated STL; its triangles and patch numbers are kept as they are)
  python simlab_mesher.py to-stl  <in.3mf> <out.stl>      (3MF from a slicer → STL in millimetres)

Outputs raw little-endian arrays plus a JSON header (see crates/core/src/mesh.rs).
Progress goes to stdout as JSON lines: {"progress": 0.4, "message": "..."}.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time

import gmsh
import numpy as np

# Gmsh's 10-node tet order differs from CalculiX/Abaqus (ours) in nodes 8 and 9.
GMSH_TET10_TO_OURS = [0, 1, 2, 3, 4, 5, 6, 7, 9, 8]


def say(progress: float, message: str) -> None:
    print(json.dumps({"progress": round(progress, 3), "message": message}), flush=True)


def is_cad(path: str) -> bool:
    return os.path.splitext(path)[1].lower() in (".step", ".stp", ".iges", ".igs", ".brep")


def setup(threads: int) -> None:
    gmsh.initialize(readConfigFiles=False)
    gmsh.option.setNumber("General.Terminal", 0)
    gmsh.option.setNumber("General.Verbosity", 2)
    gmsh.option.setNumber("General.NumThreads", threads)
    gmsh.option.setNumber("Mesh.MaxNumThreads1D", threads)
    gmsh.option.setNumber("Mesh.MaxNumThreads2D", threads)
    gmsh.option.setNumber("Mesh.MaxNumThreads3D", threads)


def load(path: str, angle_deg: float, remesh: bool, geometry: bool = True) -> str:
    """Loads the part. Returns 'cad' or 'stl' (how its patches were made).

    geometry: build Gmsh geometry from a triangle file's patches (needed to
    fill it with elements). Listing the patches does not need it, and a
    smooth closed part (a sphere, some propellers) has patches Gmsh cannot
    parametrize ("Wrong topology of boundary mesh for parametrization").
    """
    if is_cad(path):
        gmsh.option.setNumber("Geometry.OCCFixDegenerated", 1)
        gmsh.option.setNumber("Geometry.OCCFixSmallEdges", 1)
        gmsh.option.setNumber("Geometry.OCCFixSmallFaces", 1)
        gmsh.option.setNumber("Geometry.OCCMakeSolids", 1)
        # keep the file's own units: Gmsh converts STEP to millimetres by default
        gmsh.option.setString("Geometry.OCCTargetUnit", "MM")
        # Sewing first would drop every solid of a multi-body file whose
        # bodies share faces (Gmsh 4.15), so import as-is and sew only when
        # that gives no solid (loose faces, as some exporters write).
        # removeAllDuplicates fuses shared faces so bodies mesh conformally.
        for sew in (0, 1):
            gmsh.option.setNumber("Geometry.OCCSewFaces", sew)
            gmsh.model.occ.importShapes(path)
            gmsh.model.occ.removeAllDuplicates()
            gmsh.model.occ.synchronize()
            if gmsh.model.getEntities(3) or sew == 1:
                break
            gmsh.clear()
        return "cad"
    if os.path.isdir(path):
        load_surface_dir(path)
        return "discrete"
    gmsh.merge(path)
    # split the triangle soup into smooth patches; holes and flat faces come
    # out as their own patches because their edges are sharp
    gmsh.model.mesh.classifySurfaces(
        angle_deg * math.pi / 180.0,
        True,  # boundary
        remesh,  # forReparametrization (needed to remesh at a new size)
        math.pi,  # curveAngle
    )
    if geometry or remesh:
        gmsh.model.mesh.createGeometry()
        gmsh.model.geo.synchronize()
    return "stl"


def load_surface_dir(path: str) -> None:
    """Reads a surface in this script's own `surface` output format
    (surface_positions.f64, surface_triangles.u32, surface_patch.u32), e.g.
    a decimated STL from simlab_core::decimate. Each patch becomes a discrete
    Gmsh surface (tag = patch + 1) whose triangles are used as they are, and
    the closed set of them bounds one volume."""
    pos = np.fromfile(os.path.join(path, "surface_positions.f64"), dtype="<f8").reshape(-1, 3)
    tri = np.fromfile(os.path.join(path, "surface_triangles.u32"), dtype="<u4").reshape(-1, 3).astype(np.int64)
    pat = np.fromfile(os.path.join(path, "surface_patch.u32"), dtype="<u4").astype(np.int64)
    if len(pat) != len(tri):
        raise SystemExit("the surface files do not match")
    owner = np.full(len(pos), -1, dtype=np.int64)
    tags = []
    for p in np.unique(pat):
        tag = int(p) + 1
        tags.append(tag)
        gmsh.model.addDiscreteEntity(2, tag)
        t = tri[pat == p]
        used = np.unique(t)
        new = used[owner[used] < 0]
        owner[new] = tag
        if len(new):
            gmsh.model.mesh.addNodes(2, tag, (new + 1).tolist(), pos[new].ravel().tolist())
    for p in np.unique(pat):
        tag = int(p) + 1
        t = tri[pat == p]
        gmsh.model.mesh.addElementsByType(tag, 2, [], (t + 1).ravel().tolist())
    loop = gmsh.model.geo.addSurfaceLoop(tags)
    gmsh.model.geo.addVolume([loop])
    gmsh.model.geo.synchronize()


def surface_patches() -> list[int]:
    return [tag for (_, tag) in gmsh.model.getEntities(2)]


def write_raw(path: str, arr: np.ndarray, dtype) -> None:
    np.ascontiguousarray(arr, dtype=dtype).astype(dtype.newbyteorder("<")).tofile(path)


def patch_info(tag: int) -> dict:
    info = {"id": int(tag), "kind": gmsh.model.getType(2, tag)}
    try:
        info["area_mm2"] = float(gmsh.model.occ.getMass(2, tag))
    except Exception:
        pass
    return info


def cmd_surface(args) -> None:
    setup(args.threads)
    say(0.05, "Reading the part")
    kind = load(args.input, args.angle, remesh=False, geometry=False)
    patches = surface_patches()
    if kind == "cad":
        say(0.2, f"Turning {len(patches)} CAD faces into triangles")
        lo, hi = bbox()
        size = max(hi - lo)
        gmsh.option.setNumber("Mesh.MeshSizeFromCurvature", 24)
        gmsh.option.setNumber("Mesh.MeshSizeMax", size / 40.0)
        gmsh.option.setNumber("Mesh.MeshSizeMin", size / 2000.0)
        gmsh.option.setNumber("Mesh.Algorithm", 6)
        gmsh.model.mesh.generate(2)
    say(0.7, "Collecting surface patches")
    node_tags, coords, _ = gmsh.model.mesh.getNodes()
    index = np.zeros(int(node_tags.max()) + 1 if len(node_tags) else 1, dtype=np.int64)
    index[node_tags.astype(np.int64)] = np.arange(len(node_tags))
    tris = []
    tri_patch = []
    for tag in patches:
        types, _, nodes = gmsh.model.mesh.getElements(2, tag)
        for t, n in zip(types, nodes):
            if t != 2:  # 3-node triangles only
                continue
            n = index[n.astype(np.int64)].reshape(-1, 3)
            tris.append(n)
            tri_patch.append(np.full(len(n), tag, dtype=np.uint32))
    if not tris:
        raise SystemExit("no surface triangles found in the part")
    tris = np.concatenate(tris)
    tri_patch = np.concatenate(tri_patch)
    os.makedirs(args.out, exist_ok=True)
    write_raw(os.path.join(args.out, "surface_positions.f64"), coords.reshape(-1, 3), np.dtype(np.float64))
    write_raw(os.path.join(args.out, "surface_triangles.u32"), tris, np.dtype(np.uint32))
    write_raw(os.path.join(args.out, "surface_patch.u32"), tri_patch, np.dtype(np.uint32))
    header = {
        "source": kind,
        "vertices": int(len(node_tags)),
        "triangles": int(len(tris)),
        "patches": [patch_info(t) for t in patches],
        "angle_deg": args.angle,
    }
    with open(os.path.join(args.out, "surface.json"), "w") as f:
        json.dump(header, f)
    say(1.0, f"{len(tris)} triangles in {len(patches)} patches")
    gmsh.finalize()


def bbox():
    lo = np.array([math.inf] * 3)
    hi = np.array([-math.inf] * 3)
    for dim, tag in gmsh.model.getEntities(2):
        b = gmsh.model.getBoundingBox(dim, tag)
        lo = np.minimum(lo, b[:3])
        hi = np.maximum(hi, b[3:])
    return lo, hi


def cmd_volume(args) -> None:
    t0 = time.time()
    setup(args.threads)
    say(0.02, "Reading the part")
    kind = load(args.input, args.angle, remesh=args.remesh)
    patches = surface_patches()
    if kind == "stl":
        loop = gmsh.model.geo.addSurfaceLoop(patches)
        gmsh.model.geo.addVolume([loop])
        gmsh.model.geo.synchronize()
    vols = gmsh.model.getEntities(3)
    if not vols:
        raise SystemExit("the part is not a closed solid, so it cannot be filled with elements")
    size = args.size
    gmsh.option.setNumber("Mesh.MeshSizeMax", size)
    gmsh.option.setNumber("Mesh.MeshSizeMin", size / 8.0)
    gmsh.option.setNumber("Mesh.MeshSizeFromCurvature", 16 if kind == "cad" or args.remesh else 0)
    gmsh.option.setNumber("Mesh.MeshSizeExtendFromBoundary", 1)
    gmsh.option.setNumber("Mesh.Algorithm", 6)
    # HXT (parallel Delaunay) fails on a bare triangle surface; the classic
    # Delaunay mesher takes it (and is fast enough at <~2M elements)
    gmsh.option.setNumber("Mesh.Algorithm3D", 1 if kind == "discrete" else 10)
    gmsh.option.setNumber("Mesh.Optimize", 1)
    gmsh.option.setNumber("Mesh.OptimizeNetgen", 1)
    gmsh.option.setNumber("Mesh.ElementOrder", 2)
    gmsh.option.setNumber("Mesh.SecondOrderLinear", 0 if kind == "cad" else 1)
    gmsh.option.setNumber("Mesh.HighOrderOptimize", 1 if kind == "cad" else 0)
    if kind != "discrete":  # a given surface is used exactly as it is
        say(0.1, f"Meshing the surface (elements about {size:g} mm)")
        gmsh.model.mesh.generate(2)
    say(0.35, "Filling the volume with tetrahedra")
    gmsh.model.mesh.generate(3)
    say(0.8, "Making the elements quadratic")
    gmsh.model.mesh.setOrder(2)

    node_tags, coords, _ = gmsh.model.mesh.getNodes()
    index = np.zeros(int(node_tags.max()) + 1, dtype=np.int64)
    index[node_tags.astype(np.int64)] = np.arange(len(node_tags))
    tets = []
    for _, vtag in vols:
        types, _, nodes = gmsh.model.mesh.getElements(3, vtag)
        for t, n in zip(types, nodes):
            if t == 11:  # 10-node tetrahedron
                tets.append(index[n.astype(np.int64)].reshape(-1, 10)[:, GMSH_TET10_TO_OURS])
    if not tets:
        raise SystemExit("meshing produced no tetrahedra")
    tets = np.concatenate(tets)
    faces = []
    face_patch = []
    for tag in patches:
        types, _, nodes = gmsh.model.mesh.getElements(2, tag)
        for t, n in zip(types, nodes):
            if t == 9:  # 6-node triangle
                n = index[n.astype(np.int64)].reshape(-1, 6)
                faces.append(n)
                # a given surface keeps its own patch numbers (tag = patch + 1)
                face_patch.append(np.full(len(n), tag - 1 if kind == "discrete" else tag, dtype=np.uint32))
    faces = np.concatenate(faces)
    face_patch = np.concatenate(face_patch)

    # drop nodes no element uses (geometry points, orphans) and renumber
    used = np.zeros(len(node_tags), dtype=bool)
    used[tets.ravel()] = True
    remap = -np.ones(len(node_tags), dtype=np.int64)
    remap[used] = np.arange(int(used.sum()))
    coords = coords.reshape(-1, 3)[used]
    tets = remap[tets]
    faces = remap[faces]
    if (faces < 0).any():
        raise SystemExit("a surface face is not attached to the volume mesh")

    os.makedirs(args.out, exist_ok=True)
    write_raw(os.path.join(args.out, "nodes.f64"), coords, np.dtype(np.float64))
    write_raw(os.path.join(args.out, "tets.u32"), tets, np.dtype(np.uint32))
    write_raw(os.path.join(args.out, "faces.u32"), faces, np.dtype(np.uint32))
    write_raw(os.path.join(args.out, "face_patch.u32"), face_patch, np.dtype(np.uint32))
    with open(os.path.join(args.out, "mesh.json"), "w") as f:
        json.dump(
            {
                "nodes": int(len(coords)),
                "tets": int(len(tets)),
                "faces": int(len(faces)),
                "unit_m": args.unit,
                "size_mm": size,
                "seconds": round(time.time() - t0, 2),
            },
            f,
        )
    say(1.0, f"{len(tets)} elements, {len(coords)} nodes")
    gmsh.finalize()


# ------------------------------------------------------------------ 3MF

def _mat(t: str | None) -> np.ndarray:
    m = np.eye(4)
    if t:
        v = [float(x) for x in t.split()]
        m[:3, :4] = np.array(v).reshape(4, 3).T
    return m


def cmd_to_stl(args) -> None:
    """Reads a 3MF (zip of XML) with the standard library only: every build
    item, its components (also in other .model files, as PrusaSlicer and
    Bambu Studio write them) and their transforms, in millimetres."""
    import struct
    import xml.etree.ElementTree as ET
    import zipfile

    ns = {"m": "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"}
    p_ns = "{http://schemas.microsoft.com/3dmanufacturing/production/2015/06}path"
    scale_of = {"micron": 0.001, "millimeter": 1.0, "centimeter": 10.0, "inch": 25.4, "foot": 304.8, "meter": 1000.0}
    with zipfile.ZipFile(args.input) as z:
        names = [n for n in z.namelist() if n.lower().endswith(".model")]
        if sum(z.getinfo(n).file_size for n in names) > 2_000_000_000:
            raise SystemExit("this 3MF is too large")
        docs = {"/" + n.lstrip("/"): ET.fromstring(z.read(n)) for n in names}
    main = next((k for k in docs if k.lower().endswith("3d/3dmodel.model")), None)
    if main is None:
        raise SystemExit("no 3D/3dmodel.model in this 3MF")
    unit = scale_of.get(docs[main].get("unit", "millimeter"), 1.0)
    objects = {}
    for path, root in docs.items():
        for o in root.iterfind("m:resources/m:object", ns):
            objects[(path, o.get("id"))] = o
    tris = []

    def emit(path, oid, m, depth=0):
        o = objects.get((path, oid))
        if o is None or depth > 16:
            return
        mesh = o.find("m:mesh", ns)
        if mesh is not None:
            v = np.array([[float(x.get("x")), float(x.get("y")), float(x.get("z")), 1.0] for x in mesh.iterfind("m:vertices/m:vertex", ns)])
            t = np.array([[int(x.get("v1")), int(x.get("v2")), int(x.get("v3"))] for x in mesh.iterfind("m:triangles/m:triangle", ns)], dtype=np.int64)
            if len(v) and len(t):
                w = (m @ v.T).T[:, :3]
                tris.append(w[t])
        for c in o.iterfind("m:components/m:component", ns):
            emit(c.get(p_ns) or path, c.get("objectid"), m @ _mat(c.get("transform")), depth + 1)

    for item in docs[main].iterfind("m:build/m:item", ns):
        emit(item.get(p_ns) or main, item.get("objectid"), _mat(item.get("transform")))
    if not tris:
        raise SystemExit("this 3MF has no triangles")
    tri = np.concatenate(tris) * unit
    with open(args.out, "wb") as f:
        f.write(b"SimLab from 3MF".ljust(80, b" "))
        f.write(struct.pack("<I", len(tri)))
        rec = np.zeros(len(tri), dtype=[("n", "<f4", 3), ("v", "<f4", (3, 3)), ("a", "<u2")])
        n = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
        n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-30)
        rec["n"], rec["v"] = n, tri
        f.write(rec.tobytes())
    say(1.0, f"{len(tri)} triangles from the 3MF")


def main() -> None:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("surface", "volume"):
        s = sub.add_parser(name)
        s.add_argument("input")
        s.add_argument("out")
        s.add_argument("--angle", type=float, default=40.0)
        s.add_argument("--threads", type=int, default=os.cpu_count() or 4)
        if name == "volume":
            s.add_argument("--size", type=float, required=True, help="element size in model units")
            s.add_argument("--unit", type=float, default=0.001, help="metres per model unit")
            s.add_argument("--remesh", action="store_true", help="remesh STL surfaces at the new size")
    t = sub.add_parser("to-stl")
    t.add_argument("input")
    t.add_argument("out")
    t.add_argument("--threads", type=int, default=1)
    args = p.parse_args()
    try:
        {"surface": cmd_surface, "volume": cmd_volume, "to-stl": cmd_to_stl}[args.cmd](args)
    except SystemExit:
        raise
    except Exception as e:  # report in plain words, exit non-zero
        print(json.dumps({"error": str(e)}), flush=True)
        sys.exit(2)


if __name__ == "__main__":
    main()
