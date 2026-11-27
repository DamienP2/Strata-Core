// Le rendu 3D réaliste d'une carte PCIe (three.js) : circuit vernis et sa sérigraphie, composants,
// connecteur PCIe doré, refroidissement (ventilateurs, ailettes), équerre en acier découpée et ses
// prises — BNC chromées, DIN 1.0/2.3 dorées, cages SFP, RJ45, HDMI —, éclairage de studio et
// reflets (environnement), ombre portée ; caméra isométrique.
//
// Repère (mm) : X le long de l'équerre (0 = côté patte de vis), Y vers le haut (le circuit à
// y = 0), Z vers l'avant : l'équerre est à z = 0, le circuit s'étend vers −Z, les prises sortent
// vers +Z. La caméra regarde depuis (1, 0.85, 1) : l'équerre devant, la carte vers le haut à droite.
import * as THREE from 'three';
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js';

export type JackKind = 'din' | 'bnc' | 'sfp' | 'rj45';

/** L'allure d'une carte, selon son modèle. */
export interface Look {
  /** Longueur du circuit (mm). */
  length: number;
  /** Carte demi-hauteur (équerre 79 mm). */
  low?: boolean;
  /** Ventilateurs (sur un dissipateur noir). */
  fans: number;
  /** Un dissipateur plat, à ailettes basses, sans ventilateur. */
  sink?: boolean;
  /** Un grand dissipateur à hautes ailettes (cartes réseau rapides). */
  finned?: boolean;
  /** Voies PCIe (longueur du connecteur doré). */
  lanes: 1 | 4 | 8 | 16;
  /** Une prise HDMI en plus (DeckLink Mini). */
  hdmi?: boolean;
  /** Une étiquette blanche (cartes réseau). */
  sticker?: boolean;
  /** L'équerre ajourée (cartes réseau). */
  vents?: boolean;
  /** La sérigraphie du circuit (le nom du modèle). */
  silk?: string;
}

/** Une prise, de haut en bas de l'équerre. */
export interface Jack3D {
  k: number;
  /** Imprimé sur l'équerre. */
  etched: string;
  /** La couleur de sa fonction ; vide : rien. */
  color: string;
  reference?: boolean;
}

export interface Built {
  /** Le rendu, en image (le contexte WebGL est rendu aussitôt). */
  image: string;
  /** Le bout de chaque prise, en pixels CSS (pour les traits de rappel). */
  tips: { x: number; y: number }[];
  /** Hauteur et largeur du rendu, en pixels CSS. */
  width: number;
  height: number;
  /** La prise sous un point du rendu (−1 : aucune). */
  pick: (x: number, y: number) => number;
  dispose: () => void;
}

const PITCH: Record<JackKind, number> = { din: 10.5, bnc: 16.5, sfp: 15.2, rj45: 15.9 };

// ---------- Matières ----------

function materials() {
  const std = (o: THREE.MeshStandardMaterialParameters) => new THREE.MeshStandardMaterial(o);
  return {
    steel: std({ color: '#cfd3d8', metalness: 0.92, roughness: 0.32 }),
    gold: std({ color: '#e2b24f', metalness: 1, roughness: 0.22 }),
    goldDull: std({ color: '#c99a3e', metalness: 1, roughness: 0.38 }),
    chrome: std({ color: '#e6e9ee', metalness: 1, roughness: 0.14 }),
    nickel: std({ color: '#b9bec6', metalness: 1, roughness: 0.3 }),
    black: std({ color: '#141518', metalness: 0.1, roughness: 0.55 }),
    blackGloss: std({ color: '#0f1012', metalness: 0.2, roughness: 0.25 }),
    anodized: std({ color: '#1b1d21', metalness: 0.65, roughness: 0.42 }),
    alu: std({ color: '#aeb3ba', metalness: 0.95, roughness: 0.35 }),
    ptfe: std({ color: '#f1ede2', metalness: 0, roughness: 0.45 }),
    ceramic: std({ color: '#c9b48c', metalness: 0, roughness: 0.6 }),
    white: std({ color: '#f4f4f2', metalness: 0, roughness: 0.7 }),
    pcbSide: std({ color: '#0b6a2a', metalness: 0, roughness: 0.6, envMapIntensity: 0.2 }),
    copper: std({ color: '#c77d45', metalness: 1, roughness: 0.3 }),
  };
}
type Mats = ReturnType<typeof materials>;

/** La texture du dessus du circuit : vernis vert, pistes, vias, sérigraphie, trous de fixation. */
function pcbTexture(lengthMm: number, heightMm: number, silk: string): THREE.CanvasTexture {
  const ppm = 8;
  const c = document.createElement('canvas');
  c.width = Math.round(lengthMm * ppm);
  c.height = Math.round(heightMm * ppm);
  const g = c.getContext('2d')!;
  const grad = g.createLinearGradient(0, 0, c.width, c.height);
  grad.addColorStop(0, '#16a345');
  grad.addColorStop(1, '#0f8a37');
  g.fillStyle = grad;
  g.fillRect(0, 0, c.width, c.height);
  // Les pistes : des faisceaux à 45°.
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  g.lineCap = 'round';
  for (let i = 0; i < 70; i++) {
    g.strokeStyle = `rgba(10, 110, 40, ${0.35 + rnd() * 0.2})`;
    g.lineWidth = 1 + rnd() * 2;
    let x = rnd() * c.width;
    let y = rnd() * c.height;
    g.beginPath();
    g.moveTo(x, y);
    for (let s = 0; s < 4; s++) {
      const len = 20 + rnd() * 140;
      const dir = Math.floor(rnd() * 4);
      if (dir === 0) x += len;
      else if (dir === 1) y += len;
      else if (dir === 2) {
        x += len * 0.7;
        y += len * 0.7;
      } else {
        x += len * 0.7;
        y -= len * 0.7;
      }
      g.lineTo(x, y);
    }
    g.stroke();
  }
  // Les vias.
  for (let i = 0; i < 260; i++) {
    const x = rnd() * c.width;
    const y = rnd() * c.height;
    g.fillStyle = 'rgba(214, 178, 96, .85)';
    g.beginPath();
    g.arc(x, y, 2.2, 0, Math.PI * 2);
    g.fill();
    g.fillStyle = '#0e3a1c';
    g.beginPath();
    g.arc(x, y, 1, 0, Math.PI * 2);
    g.fill();
  }
  // La sérigraphie : des contours de composants, des repères, le nom du modèle.
  g.strokeStyle = 'rgba(245, 245, 240, .75)';
  g.fillStyle = 'rgba(245, 245, 240, .8)';
  g.lineWidth = 1.4;
  g.font = `${Math.round(ppm * 1.6)}px sans-serif`;
  for (let i = 0; i < 40; i++) {
    const x = rnd() * c.width;
    const y = rnd() * c.height;
    g.strokeRect(x, y, 10 + rnd() * 18, 6 + rnd() * 10);
    if (rnd() > 0.5) g.fillText(`${'RCULQ'[Math.floor(rnd() * 5)]}${Math.floor(rnd() * 400)}`, x, y - 3);
  }
  if (silk) {
    g.font = `bold ${Math.round(ppm * 3.2)}px sans-serif`;
    g.fillText(silk, c.width * 0.55, c.height * 0.9);
  }
  // Les trous de fixation dorés.
  for (const [x, y] of [
    [c.width - 30, 30],
    [c.width - 30, c.height - 30],
  ]) {
    g.fillStyle = '#d9b05a';
    g.beginPath();
    g.arc(x, y, 18, 0, Math.PI * 2);
    g.fill();
    g.fillStyle = '#0b0c0d';
    g.beginPath();
    g.arc(x, y, 10, 0, Math.PI * 2);
    g.fill();
  }
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = 8;
  return t;
}

/** Un texte sur fond transparent, à poser comme un décalque. */
function label(text: string, w: number, h: number, color = '#1b1d22', bold = true): THREE.Mesh {
  const ppm = 24;
  const c = document.createElement('canvas');
  c.width = Math.round(w * ppm);
  c.height = Math.round(h * ppm);
  const g = c.getContext('2d')!;
  g.fillStyle = color;
  g.font = `${bold ? 'bold ' : ''}${Math.round(h * ppm * 0.82)}px Inter, Arial, sans-serif`;
  g.textAlign = 'center';
  g.textBaseline = 'middle';
  g.fillText(text, c.width / 2, c.height / 2 + 1);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  const m = new THREE.Mesh(new THREE.PlaneGeometry(w, h), new THREE.MeshStandardMaterial({ map: t, transparent: true, roughness: 0.8, metalness: 0 }));
  return m;
}

/** Une boîte posée : coins (x0, y0, z0) et (x1, y1, z1). */
function box(m: THREE.Material | THREE.Material[], x0: number, x1: number, y0: number, y1: number, z0: number, z1: number): THREE.Mesh {
  const mesh = new THREE.Mesh(new THREE.BoxGeometry(Math.abs(x1 - x0), Math.abs(y1 - y0), Math.abs(z1 - z0)), m);
  mesh.position.set((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
  mesh.castShadow = true;
  mesh.receiveShadow = true;
  return mesh;
}

/** Un cylindre d'axe Z (vers l'avant), de z0 à z1. */
function cylZ(m: THREE.Material, r: number, x: number, y: number, z0: number, z1: number, seg = 32, rTop = r): THREE.Mesh {
  const mesh = new THREE.Mesh(new THREE.CylinderGeometry(rTop, r, Math.abs(z1 - z0), seg), m);
  mesh.rotation.x = Math.PI / 2;
  mesh.position.set(x, y, (z0 + z1) / 2);
  mesh.castShadow = true;
  return mesh;
}

/** Un profil tourné autour de l'axe Z : points (rayon, z). */
function latheZ(m: THREE.Material, profile: [number, number][], x: number, y: number, seg = 48): THREE.Mesh {
  const g = new THREE.LatheGeometry(
    profile.map(([r, z]) => new THREE.Vector2(r, z)),
    seg,
  );
  const mesh = new THREE.Mesh(g, m);
  // LatheGeometry tourne autour de Y : on couche Y sur Z.
  mesh.rotation.x = Math.PI / 2;
  mesh.position.set(x, y, 0);
  mesh.castShadow = true;
  return mesh;
}

// ---------- Les prises ----------

function bnc(M: Mats, x: number, y: number, ref: boolean): THREE.Group {
  const g = new THREE.Group();
  const s = ref ? 0.86 : 1;
  // L'écrou hexagonal contre l'équerre, le fût, la bague à baïonnette, l'isolant, l'âme.
  const nut = cylZ(M.nickel, 6.4 * s, x, y, 0, 2.4, 6);
  nut.rotation.y = Math.PI / 6;
  g.add(nut);
  g.add(latheZ(M.chrome, [[0, 2.4], [5.1 * s, 2.4], [5.1 * s, 3.2], [4.7 * s, 3.6], [4.7 * s, 11], [5.0 * s, 11.4], [5.0 * s, 13.6], [4.6 * s, 14.2], [3.6 * s, 14.2], [3.6 * s, 13.2], [0, 13.2]], x, y));
  // Les ergots de la baïonnette.
  for (const side of [-1, 1]) g.add(cylZ(M.chrome, 0.9, x + side * 5.3 * s, y, 11.2, 12.8, 16));
  g.add(cylZ(M.ptfe, 3.3 * s, x, y, 12.6, 13.6));
  g.add(cylZ(M.gold, 0.55, x, y, 12.8, 14.0, 16));
  return g;
}

function din(M: Mats, x: number, y: number, ref: boolean): THREE.Group {
  const g = new THREE.Group();
  const s = ref ? 0.88 : 1;
  // L'écrou à six pans, le corps doré moleté (stries), le bout, l'isolant, l'âme.
  const nut = cylZ(M.gold, 3.9 * s, x, y, 0, 1.6, 6);
  nut.rotation.y = Math.PI / 6;
  g.add(nut);
  g.add(latheZ(M.gold, [[0, 1.6], [2.9 * s, 1.6], [2.9 * s, 2.2], [2.55 * s, 2.5], [2.55 * s, 7.6], [2.75 * s, 8], [2.75 * s, 9.4], [2.1 * s, 9.6], [1.55 * s, 9.6], [1.55 * s, 9.0], [0, 9.0]], x, y));
  for (let i = 0; i < 18; i++) {
    const a = (i / 18) * Math.PI * 2;
    const rib = box(M.goldDull, -0.12, 0.12, -0.12, 0.12, 3.0, 7.2);
    rib.position.set(x + Math.cos(a) * 2.6 * s, y + Math.sin(a) * 2.6 * s, 5.1);
    g.add(rib);
  }
  g.add(cylZ(M.ptfe, 1.45 * s, x, y, 8.6, 9.1));
  g.add(cylZ(M.gold, 0.3, x, y, 8.8, 9.6, 12));
  return g;
}

/** Une cage SFP : sa façade (ouverture, ressorts), sa coque sur le circuit, perforée. */
function sfp(M: Mats, x: number, w: number, depth: number): THREE.Group {
  const g = new THREE.Group();
  const h = 9.8;
  const y0 = 0.4;
  // La coque, sur le circuit, derrière l'équerre.
  g.add(box(M.alu, x - w / 2, x + w / 2, y0, y0 + h, -depth, 0));
  // Les perforations de la coque.
  for (let i = 0; i < 4; i++)
    for (let j = 0; j < 2; j++) {
      const hole = new THREE.Mesh(new THREE.CircleGeometry(0.75, 16), M.black);
      hole.rotation.x = -Math.PI / 2;
      hole.position.set(x - w / 4 + (j * w) / 2, y0 + h + 0.02, -8 - i * 8);
      g.add(hole);
    }
  // La façade qui dépasse de l'équerre, son ouverture sombre, ses ressorts.
  g.add(box(M.nickel, x - w / 2 - 0.4, x + w / 2 + 0.4, y0 - 0.4, y0 + h + 0.4, 0, 3.2));
  g.add(box(M.blackGloss, x - w / 2 + 1.0, x + w / 2 - 1.0, y0 + 1.1, y0 + h - 1.1, 2.5, 3.25));
  for (let i = 0; i < 5; i++) g.add(box(M.chrome, x - w / 2 + 1.6 + i * ((w - 3.2) / 4) - 0.35, x - w / 2 + 1.6 + i * ((w - 3.2) / 4) + 0.35, y0 + h + 0.4, y0 + h + 0.9, 0.4, 3.0));
  // Le module 25G est absent : on voit la languette de la cage.
  g.add(box(M.nickel, x - 1.6, x + 1.6, y0 + 3.4, y0 + 5.4, 2.4, 2.6));
  return g;
}

function rj45(M: Mats, x: number, w: number, lit: boolean): THREE.Group {
  const g = new THREE.Group();
  const h = 13.2;
  g.add(box(M.nickel, x - w / 2, x + w / 2, 0.4, 0.4 + h, -21, 2.2));
  g.add(box(M.blackGloss, x - w / 2 + 1.1, x + w / 2 - 1.1, 1.6, 10.2, 1.7, 2.25));
  // La languette en bas de l'ouverture, les voyants en haut.
  g.add(box(M.black, x - 1.8, x + 1.8, 1.6, 3.0, 1.0, 2.2));
  const led = (c: string, on: boolean) => new THREE.MeshStandardMaterial({ color: c, emissive: on ? c : '#000000', emissiveIntensity: on ? 1.4 : 0, roughness: 0.3 });
  g.add(box(led('#2ee86b', lit), x - w / 2 + 1.2, x - w / 2 + 3.4, 11.2, 12.6, 2.0, 2.3));
  g.add(box(led('#ffb02e', lit), x + w / 2 - 3.4, x + w / 2 - 1.2, 11.2, 12.6, 2.0, 2.3));
  return g;
}

function hdmi(M: Mats, x: number, y: number): THREE.Group {
  const g = new THREE.Group();
  g.add(box(M.nickel, x - 7.5, x + 7.5, y - 3, y + 3, -10, 1.2));
  g.add(box(M.blackGloss, x - 6.6, x + 6.6, y - 2.1, y + 2.1, 0.5, 1.25));
  g.add(box(M.black, x - 5.2, x + 5.2, y - 0.6, y + 0.6, 0.2, 1.1));
  return g;
}

/** Un anneau lumineux autour d'une prise ronde (la couleur de sa fonction). */
function ring(color: string, x: number, y: number, z: number, r: number, width = 0.55): THREE.Mesh {
  const m = new THREE.Mesh(
    new THREE.TorusGeometry(r, width, 12, 64),
    new THREE.MeshStandardMaterial({ color, emissive: color, emissiveIntensity: 1.1, roughness: 0.4 }),
  );
  m.position.set(x, y, z);
  return m;
}

/** Un cadre lumineux autour d'une prise carrée. */
function frame(color: string, x0: number, x1: number, y0: number, y1: number, z: number): THREE.Group {
  const g = new THREE.Group();
  const mat = new THREE.MeshStandardMaterial({ color, emissive: color, emissiveIntensity: 1.1, roughness: 0.4 });
  const t = 0.6;
  for (const [a0, a1, b0, b1] of [
    [x0 - t, x1 + t, y0 - t, y0],
    [x0 - t, x1 + t, y1, y1 + t],
    [x0 - t, x0, y0, y1],
    [x1, x1 + t, y0, y1],
  ]) {
    const m = box(mat, a0, a1, b0, b1, z, z + 0.4);
    m.castShadow = false;
    g.add(m);
  }
  return g;
}

// ---------- Refroidissement ----------

/** Un ventilateur : cadre, pales courbes inclinées, moyeu et son étiquette. */
function fan(M: Mats, cx: number, cz: number, y: number, r: number): THREE.Group {
  const g = new THREE.Group();
  // Le cadre carré évidé.
  const shape = new THREE.Shape();
  const s = r + 3;
  shape.moveTo(-s, -s);
  shape.lineTo(s, -s);
  shape.lineTo(s, s);
  shape.lineTo(-s, s);
  shape.lineTo(-s, -s);
  const hole = new THREE.Path();
  hole.absarc(0, 0, r + 0.6, 0, Math.PI * 2, true);
  shape.holes.push(hole);
  const frameMesh = new THREE.Mesh(new THREE.ExtrudeGeometry(shape, { depth: 4, bevelEnabled: true, bevelSize: 0.6, bevelThickness: 0.6, bevelSegments: 2 }), M.black);
  frameMesh.rotation.x = -Math.PI / 2;
  frameMesh.position.set(cx, y, cz);
  frameMesh.castShadow = true;
  g.add(frameMesh);
  // Les pales.
  const blade = new THREE.Shape();
  blade.moveTo(r * 0.28, -1.4);
  blade.quadraticCurveTo(r * 0.7, -3.4, r * 0.96, -1.2);
  blade.lineTo(r * 0.96, 2.6);
  blade.quadraticCurveTo(r * 0.6, 1.4, r * 0.28, 2.2);
  blade.lineTo(r * 0.28, -1.4);
  const bladeGeo = new THREE.ExtrudeGeometry(blade, { depth: 0.6, bevelEnabled: false });
  for (let i = 0; i < 9; i++) {
    const b = new THREE.Mesh(bladeGeo, M.blackGloss);
    const pivot = new THREE.Group();
    b.rotation.x = 0.42;
    pivot.add(b);
    pivot.rotation.x = -Math.PI / 2;
    pivot.rotation.z = (i / 9) * Math.PI * 2;
    pivot.position.set(cx, y + 2.2, cz);
    b.castShadow = true;
    g.add(pivot);
  }
  // Le moyeu et son étiquette.
  const hub = cylZ(M.blackGloss, r * 0.3, cx, 0, 0, 3.6);
  hub.rotation.x = 0;
  hub.position.set(cx, y + 2.4, cz);
  g.add(hub);
  const sticker = new THREE.Mesh(new THREE.CircleGeometry(r * 0.22, 32), new THREE.MeshStandardMaterial({ color: '#2a2d33', roughness: 0.5, metalness: 0.3 }));
  sticker.rotation.x = -Math.PI / 2;
  sticker.position.set(cx, y + 4.22, cz);
  g.add(sticker);
  return g;
}

/** Un dissipateur à ailettes, d'un seul tenant (ailettes en instances). */
function finned(M: Mats, x0: number, x1: number, z0: number, z1: number, base: number, top: number, pitch: number, along: 'z' | 'x'): THREE.Group {
  const g = new THREE.Group();
  g.add(box(M.anodized, x0, x1, 0.2, base, z0, z1));
  const thick = 0.9;
  const n = Math.floor(((along === 'z' ? x1 - x0 : z1 - z0) - thick) / pitch) + 1;
  const geo = along === 'z' ? new THREE.BoxGeometry(thick, top - base, Math.abs(z1 - z0)) : new THREE.BoxGeometry(Math.abs(x1 - x0), top - base, thick);
  const fins = new THREE.InstancedMesh(geo, M.anodized, n);
  const m = new THREE.Matrix4();
  for (let i = 0; i < n; i++) {
    if (along === 'z') m.makeTranslation(x0 + thick / 2 + i * pitch, (base + top) / 2, (z0 + z1) / 2);
    else m.makeTranslation((x0 + x1) / 2, (base + top) / 2, z0 + thick / 2 + i * pitch);
    fins.setMatrixAt(i, m);
  }
  fins.castShadow = true;
  fins.receiveShadow = true;
  g.add(fins);
  return g;
}

// ---------- La carte ----------

export function buildScene(look: Look, kind: JackKind, jacks: Jack3D[], widthCss: number, labelsHeight: number): Built {
  const M = materials();
  const scene = new THREE.Scene();
  const card = new THREE.Group();
  scene.add(card);

  const round = kind === 'din' || kind === 'bnc';
  const bracketLen = look.low ? 79.2 : 120;
  const H = look.low ? 64 : 98; // le circuit, le long de l'équerre
  const bx0 = 4;
  const bx1 = bx0 + H;
  const L = look.length;
  const nJacks = jacks.length + (look.hdmi ? 1 : 0);
  const first = round ? 11 : 13;
  const pitch = PITCH[kind];
  const at = (i: number) => first + i * pitch + (kind === 'sfp' || kind === 'rj45' ? pitch / 2 : 0);
  const yc = round ? (kind === 'din' ? 6.8 : 7.6) : 0;

  // Le circuit : dessus texturé, tranches vertes.
  const tex = pcbTexture(L, H, look.silk ?? '');
  tex.rotation = Math.PI / 2;
  tex.center.set(0.5, 0.5);
  // Le vernis épargne : un peu brillant, sans délaver le vert.
  const top = new THREE.MeshPhysicalMaterial({ map: tex, color: new THREE.Color().setRGB(0.3, 0.68, 0.34, THREE.LinearSRGBColorSpace), roughness: 0.62, clearcoat: 0.25, clearcoatRoughness: 0.4, envMapIntensity: 0.12 });
  const pcb = box([M.pcbSide, M.pcbSide, top, M.pcbSide, M.pcbSide, M.pcbSide], bx0, bx1, -1.6, 0, -L, 0);
  card.add(pcb);

  // Le connecteur PCIe : la languette, ses contacts dorés, l'encoche.
  const fingerLen = { 1: 25, 4: 39, 8: 56, 16: 89 }[look.lanes];
  const f0 = -Math.min(L - 10, 30 + fingerLen);
  card.add(box(M.pcbSide, bx1, bx1 + 7.2, -1.6, 0, f0, f0 + fingerLen + 12));
  // Un contact par millimètre, sauf dans l'encoche (de 11 à 12,6 mm).
  const contactZ: number[] = [];
  for (let z = f0 + 1; z < f0 + fingerLen + 11; z += 1.0) if (!(z > f0 + 11 && z < f0 + 12.6)) contactZ.push(z);
  const contacts = new THREE.InstancedMesh(new THREE.BoxGeometry(5.6, 0.05, 0.7), M.gold, contactZ.length);
  const mtx = new THREE.Matrix4();
  contactZ.forEach((z, i) => {
    mtx.makeTranslation(bx1 + 4, 0.03, z);
    contacts.setMatrixAt(i, mtx);
  });
  card.add(contacts);

  // Des composants CMS : résistances, condensateurs, puces, selfs, condensateurs chimiques.
  let seed = 11;
  const rnd = () => ((seed = (seed * 48271) % 2147483647) / 2147483647);
  const keepOut = (x: number, z: number) => z > -20 || (look.finned && z < -(round ? 26 : 50) && x > bx0 + 3 && x < bx1 - 3);
  for (let i = 0; i < 140; i++) {
    const x = bx0 + 3 + rnd() * (H - 6);
    const z = -8 - rnd() * (L - 14);
    if (keepOut(x, z)) continue;
    const r = rnd();
    if (r < 0.55) {
      const w = 1 + rnd() * 1.2;
      card.add(box(r < 0.3 ? M.black : M.ceramic, x, x + w, 0, 0.5, z, z + w * 0.5));
      card.add(box(M.nickel, x - 0.15, x + 0.05, 0, 0.5, z, z + w * 0.5));
      card.add(box(M.nickel, x + w - 0.05, x + w + 0.15, 0, 0.5, z, z + w * 0.5));
    } else if (r < 0.75) {
      const w = 3 + rnd() * 4;
      card.add(box(M.black, x, x + w, 0, 1.1, z, z + w));
    } else if (r < 0.9) {
      card.add(box(M.anodized, x, x + 4, 0, 2.6, z, z + 4));
    } else {
      const cap = new THREE.Mesh(new THREE.CylinderGeometry(1.9, 1.9, 4.2, 24), M.alu);
      cap.position.set(x, 2.1, z);
      cap.castShadow = true;
      card.add(cap);
    }
  }

  // Le refroidissement.
  if (look.finned) {
    const z1 = round ? -26 : -50;
    card.add(finned(M, bx0 + 4, bx1 - 4, -L + 6, z1, 2.4, 14.5, 2.9, 'z'));
    // Les vis de fixation du dissipateur.
    for (const [x, z] of [
      [bx0 + 20, z1 - 10],
      [bx1 - 20, -L + 20],
    ])
      card.add(cylZ(M.blackGloss, 1.8, x, 15, z, z + 0.1));
  } else if (look.fans > 0 || look.sink) {
    const n = Math.max(look.fans, 1);
    const span = Math.min(L - 46, n * 56);
    const z0 = -L + (L - span) / 2 - 8;
    const x0 = bx0 + 6;
    const x1 = bx1 - 6;
    if (look.fans) {
      card.add(finned(M, x0, x1, z0, z0 + span, 2.2, 6.4, 2.4, 'x'));
      // Le capot noir qui coiffe les ailettes, ses ouvertures pour les ventilateurs.
      card.add(box(M.anodized, x0, x1, 6.4, 7.2, z0, z0 + span));
      for (let i = 0; i < look.fans; i++) {
        const r = Math.min((x1 - x0) / 2 - 4, span / look.fans / 2 - 4);
        card.add(fan(M, (x0 + x1) / 2, z0 + (span * (i + 0.5)) / look.fans, 7.2, r));
      }
    } else {
      card.add(finned(M, x0, x1, z0, z0 + span, 1.6, 7.5, 2.6, 'x'));
    }
  }

  // L'étiquette blanche, sur les cages (cartes réseau).
  if (look.sticker && !round) {
    const st = box(M.white, at(0) - 6, at(nJacks - 1) + 6, 10.25, 10.3, -40, -22);
    card.add(st);
    const lines = new THREE.InstancedMesh(new THREE.BoxGeometry(0.35, 0.02, 4), M.black, 40);
    for (let i = 0; i < 40; i++) {
      mtx.makeTranslation(at(0) - 4 + i * 0.6 * (1 + (i % 3) * 0.3), 10.32, -29);
      lines.setMatrixAt(i, mtx);
    }
    card.add(lines);
  }

  // L'équerre : une tôle d'acier découpée (ses trous), sa patte de vis repliée, sa languette.
  const ext = 18.4;
  const ey0 = -3.2;
  const shape = new THREE.Shape();
  shape.moveTo(-2, ey0);
  shape.lineTo(bracketLen, ey0);
  shape.lineTo(bracketLen, ey0 + ext);
  shape.lineTo(-2, ey0 + ext);
  shape.lineTo(-2, ey0);
  jacks.forEach((j, i) => {
    const x = at(i);
    const hole = new THREE.Path();
    if (round) hole.absarc(x, yc, (kind === 'din' ? 2.6 : 4.9) * (j.reference ? 0.88 : 1), 0, Math.PI * 2, true);
    else {
      const w = (pitch - 1.4) / 2;
      const top2 = kind === 'sfp' ? 10.6 : 14;
      hole.moveTo(x - w, 0);
      hole.lineTo(x - w, top2);
      hole.lineTo(x + w, top2);
      hole.lineTo(x + w, 0);
      hole.lineTo(x - w, 0);
    }
    shape.holes.push(hole);
  });
  if (look.hdmi) {
    const x = at(jacks.length) + 4;
    const hole = new THREE.Path();
    hole.moveTo(x - 7.8, yc - 3.3);
    hole.lineTo(x + 7.8, yc - 3.3);
    hole.lineTo(x + 7.8, yc + 3.3);
    hole.lineTo(x - 7.8, yc + 3.3);
    hole.lineTo(x - 7.8, yc - 3.3);
    shape.holes.push(hole);
  }
  if (look.vents) {
    const from = at(nJacks - 1) + pitch;
    for (let x = from; x < bracketLen - 6; x += 4.2)
      for (const y of [2, 6.2, 10.4]) {
        const v = new THREE.Path();
        v.absarc(x, y, 1.3, 0, Math.PI * 2, true);
        shape.holes.push(v);
      }
  }
  const plate = new THREE.Mesh(new THREE.ExtrudeGeometry(shape, { depth: 0.8, bevelEnabled: true, bevelSize: 0.25, bevelThickness: 0.2, bevelSegments: 2, curveSegments: 36 }), M.steel);
  plate.position.z = -0.8;
  plate.castShadow = true;
  plate.receiveShadow = true;
  card.add(plate);
  // La patte repliée en haut, avec sa fente de vis.
  const tab = new THREE.Shape();
  tab.moveTo(0, 0);
  tab.lineTo(11, 0);
  tab.lineTo(11, ext + 2.4);
  tab.lineTo(0, ext + 2.4);
  tab.lineTo(0, 0);
  const slot = new THREE.Path();
  slot.moveTo(3.2, 7);
  slot.lineTo(7.8, 7);
  slot.absarc(7.8, 9.2, 2.2, -Math.PI / 2, Math.PI / 2, false);
  slot.lineTo(3.2, 11.4);
  slot.absarc(3.2, 9.2, 2.2, Math.PI / 2, (3 * Math.PI) / 2, false);
  tab.holes.push(slot);
  const tabMesh = new THREE.Mesh(new THREE.ExtrudeGeometry(tab, { depth: 0.8, bevelEnabled: false }), M.steel);
  // Dans le plan x = −2 : la forme x → vers le fond (−Z), y → vers le haut.
  tabMesh.rotation.y = Math.PI / 2;
  tabMesh.position.set(-2.8, ey0 - 1.2, 0);
  tabMesh.castShadow = true;
  card.add(tabMesh);
  // La languette du bas, qui entre dans la fente du châssis.
  card.add(box(M.steel, bracketLen - 1, bracketLen + 6, ey0 + 5, ey0 + 12, -0.8, 0));

  // Les prises, leurs numéros imprimés, et l'anneau de leur fonction.
  const tipsWorld: THREE.Vector3[] = [];
  const pickables: THREE.Object3D[] = [];
  jacks.forEach((j, i) => {
    const x = at(i);
    let g: THREE.Group;
    let tip: THREE.Vector3;
    if (kind === 'bnc') {
      g = bnc(M, x, yc, !!j.reference);
      tip = new THREE.Vector3(x, yc - 5, 14.2);
      if (j.color) g.add(ring(j.color, x, yc, 14.3, 4.8 * (j.reference ? 0.86 : 1)));
    } else if (kind === 'din') {
      g = din(M, x, yc, !!j.reference);
      tip = new THREE.Vector3(x, yc - 2.8, 9.6);
      if (j.color) g.add(ring(j.color, x, yc, 9.7, 2.7 * (j.reference ? 0.88 : 1), 0.4));
    } else if (kind === 'sfp') {
      g = sfp(M, x, pitch - 1.6, 44);
      tip = new THREE.Vector3(x, 0, 3.2);
      if (j.color) g.add(frame(j.color, x - (pitch - 1.6) / 2 - 0.4, x + (pitch - 1.6) / 2 + 0.4, 0, 10.6, 3.2));
    } else {
      g = rj45(M, x, pitch - 1.4, !!j.color);
      tip = new THREE.Vector3(x, 0.4, 2.2);
      if (j.color) g.add(frame(j.color, x - (pitch - 1.4) / 2, x + (pitch - 1.4) / 2, 0.4, 13.6, 2.2));
    }
    g.userData.k = j.k;
    g.traverse((o) => (o.userData.k = j.k));
    card.add(g);
    pickables.push(g);
    tipsWorld.push(tip);
    // Le numéro imprimé sur l'équerre, au-dessus (ou à côté) de la prise.
    // DIN et SFP : au-dessus ; BNC et RJ45 (grands) : en dessous, sous le niveau du circuit.
    const t = label(j.etched, j.etched.length > 2 ? 7 : 4, 2.6);
    t.position.set(x, kind === 'din' || kind === 'sfp' ? ey0 + ext - 2.1 : ey0 + 1.7, 0.42);
    card.add(t);
  });
  if (look.hdmi) card.add(hdmi(M, at(jacks.length) + 4, yc));

  // Le sol, qui reçoit l'ombre.
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(800, 800), new THREE.ShadowMaterial({ opacity: 0.28 }));
  ground.rotation.x = -Math.PI / 2;
  ground.position.y = ey0 - 1.6;
  ground.receiveShadow = true;
  scene.add(ground);

  // L'éclairage : un environnement de studio (reflets), une lumière principale qui porte l'ombre,
  // un contre-jour léger.
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.05;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  const pmrem = new THREE.PMREMGenerator(renderer);
  const env = pmrem.fromScene(new RoomEnvironment(), 0.03).texture;
  scene.environment = env;
  scene.environmentIntensity = 0.85;
  const key = new THREE.DirectionalLight('#ffffff', 1.7);
  key.position.set(60, 220, 140);
  key.castShadow = true;
  key.shadow.mapSize.set(2048, 2048);
  key.shadow.camera.left = -200;
  key.shadow.camera.right = 200;
  key.shadow.camera.top = 200;
  key.shadow.camera.bottom = -200;
  key.shadow.camera.far = 600;
  key.shadow.bias = -0.0004;
  key.shadow.radius = 6;
  scene.add(key);
  const rim = new THREE.DirectionalLight('#cfe0ff', 0.7);
  rim.position.set(-160, 120, -200);
  scene.add(rim);

  // La caméra isométrique, cadrée sur la carte (et ses prises), de la place en bas pour les noms.
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 1, 4000);
  const dir = new THREE.Vector3(1, 0.85, 1).normalize();
  const bboxCard = new THREE.Box3().setFromObject(card);
  const center = bboxCard.getCenter(new THREE.Vector3());
  camera.position.copy(center.clone().add(dir.clone().multiplyScalar(1200)));
  camera.up.set(0, 1, 0);
  camera.lookAt(center);
  camera.updateMatrixWorld();
  const corners: THREE.Vector3[] = [];
  for (const x of [bboxCard.min.x, bboxCard.max.x])
    for (const y of [bboxCard.min.y, bboxCard.max.y]) for (const z of [bboxCard.min.z, bboxCard.max.z]) corners.push(new THREE.Vector3(x, y, z).applyMatrix4(camera.matrixWorldInverse));
  const minX = Math.min(...corners.map((c) => c.x));
  const maxX = Math.max(...corners.map((c) => c.x));
  const minY = Math.min(...corners.map((c) => c.y));
  const maxY = Math.max(...corners.map((c) => c.y));
  const pad = 4;
  const worldW = maxX - minX + 2 * pad;
  const pxPerMm = widthCss / worldW;
  const labelMm = labelsHeight / pxPerMm;
  const worldH = maxY - minY + 2 * pad + labelMm;
  camera.left = minX - pad;
  camera.right = maxX + pad;
  camera.top = maxY + pad;
  camera.bottom = minY - pad - labelMm;
  camera.updateProjectionMatrix();
  const heightCss = Math.round(worldH * pxPerMm);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.setSize(widthCss, heightCss, false);
  renderer.render(scene, camera);
  const image = renderer.domElement.toDataURL('image/png');
  env.dispose();
  pmrem.dispose();
  renderer.dispose();
  renderer.forceContextLoss();

  const toCss = (v: THREE.Vector3) => {
    const p = v.clone().project(camera);
    return { x: ((p.x + 1) / 2) * widthCss, y: ((1 - p.y) / 2) * heightCss };
  };
  const raycaster = new THREE.Raycaster();
  return {
    image,
    tips: tipsWorld.map(toCss),
    width: widthCss,
    height: heightCss,
    pick: (x, y) => {
      raycaster.setFromCamera(new THREE.Vector2((x / widthCss) * 2 - 1, 1 - (y / heightCss) * 2), camera);
      const hit = raycaster.intersectObjects(pickables, true)[0];
      return hit ? (hit.object.userData.k ?? -1) : -1;
    },
    dispose: () => {
      scene.traverse((o) => {
        const m = o as THREE.Mesh;
        m.geometry?.dispose();
        const mat = m.material as THREE.Material | THREE.Material[] | undefined;
        (Array.isArray(mat) ? mat : mat ? [mat] : []).forEach((x) => {
          (x as THREE.MeshStandardMaterial).map?.dispose();
          x.dispose();
        });
      });
    },
  };
}
