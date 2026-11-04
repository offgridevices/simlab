// Small still pictures of parts for the projects list, drawn one at a time
// by a single hidden renderer (one GPU context, however many projects).
import * as THREE from "three";
import type { Api } from "../lib/api";

const cache = new Map<string, Promise<string | null>>();
let queue: Promise<unknown> = Promise.resolve();
let renderer: THREE.WebGLRenderer | null = null;
/** parts bigger than this get an icon instead: their surface is a big download for a 48-pixel picture */
const MAX_TRIANGLES = 400_000;

export function thumbnail(api: Api, modelId: string, colors: { part: string; dark: boolean }): Promise<string | null> {
  const key = `${modelId}:${colors.part}:${colors.dark}`;
  let p = cache.get(key);
  if (!p) {
    p = (queue = queue.then(() => draw(api, modelId, colors), () => draw(api, modelId, colors))) as Promise<string | null>;
    p.catch(() => cache.delete(key));
    cache.set(key, p);
  }
  return p;
}

async function draw(api: Api, id: string, colors: { part: string; dark: boolean }): Promise<string | null> {
  const model = await api.model(id);
  if (model.status !== "ready" || (model.triangles ?? Infinity) > MAX_TRIANGLES) return null;
  const s = await api.surface(id);
  const size = 160;
  renderer ??= new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
  renderer.setSize(size, size, false);
  renderer.setPixelRatio(1);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.setClearColor(0x000000, 0);
  const scene = new THREE.Scene();
  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.BufferAttribute(s.positions, 3));
  g.setAttribute("normal", new THREE.BufferAttribute(s.normals, 3));
  g.setIndex(new THREE.BufferAttribute(s.indices, 1));
  g.computeBoundingSphere();
  const mat = new THREE.MeshStandardMaterial({ color: new THREE.Color().setStyle(colors.part || "#b9b0a0", THREE.SRGBColorSpace), roughness: 0.6, side: THREE.DoubleSide });
  scene.add(new THREE.Mesh(g, mat));
  scene.add(new THREE.HemisphereLight(colors.dark ? 0xf1ece0 : 0xffffff, colors.dark ? 0x2a241d : 0x8a8478, 1.4));
  const sun = new THREE.DirectionalLight(0xffffff, 1.6);
  const cam = new THREE.PerspectiveCamera(30, 1, 0.1, 1e5);
  cam.up.set(0, 0, 1);
  const { center, radius } = g.boundingSphere!;
  const dir = new THREE.Vector3(0.85, -1.15, 0.8).normalize();
  cam.position.copy(center).addScaledVector(dir, (radius / Math.sin(THREE.MathUtils.degToRad(15))) * 1.02);
  cam.near = radius / 100;
  cam.far = radius * 100;
  cam.lookAt(center);
  cam.updateProjectionMatrix();
  sun.position.copy(cam.position).add(new THREE.Vector3(radius, radius, radius * 2));
  scene.add(sun);
  renderer.render(scene, cam);
  const url = renderer.domElement.toDataURL("image/png");
  g.dispose();
  mat.dispose();
  return url;
}
