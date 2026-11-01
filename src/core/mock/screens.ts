// Draws a fake "LeetCode in a browser" screenshot on a canvas for the mock.

function drawScreen(ctx: CanvasRenderingContext2D, w: number, h: number) {
  const s = w / 1568;
  ctx.fillStyle = "#1a1a1a";
  ctx.fillRect(0, 0, w, h);
  // browser chrome
  ctx.fillStyle = "#2b2b2b";
  ctx.fillRect(0, 0, w, 64 * s);
  ctx.fillStyle = "#3a3a3a";
  ctx.beginPath();
  ctx.roundRect(140 * s, 16 * s, 900 * s, 32 * s, 16 * s);
  ctx.fill();
  ctx.fillStyle = "#bdbdbd";
  ctx.font = `${16 * s}px Segoe UI, sans-serif`;
  ctx.fillText("leetcode.com/problems/two-sum", 162 * s, 38 * s);
  // left: problem
  const left = 24 * s;
  ctx.fillStyle = "#262626";
  ctx.fillRect(left, 84 * s, 720 * s, h - 104 * s);
  ctx.fillStyle = "#f5f5f5";
  ctx.font = `600 ${30 * s}px Segoe UI, sans-serif`;
  ctx.fillText("1. Two Sum", left + 28 * s, 140 * s);
  ctx.fillStyle = "#3fa66b";
  ctx.font = `${17 * s}px Segoe UI, sans-serif`;
  ctx.fillText("Easy", left + 28 * s, 176 * s);
  ctx.fillStyle = "#d4d4d4";
  ctx.font = `${19 * s}px Segoe UI, sans-serif`;
  const lines = [
    "Given an array of integers nums and an integer target,",
    "return indices of the two numbers such that they add up",
    "to target.",
    "",
    "You may assume that each input would have exactly one",
    "solution, and you may not use the same element twice.",
    "",
    "Example 1:",
    "  Input: nums = [2,7,11,15], target = 9",
    "  Output: [0,1]",
    "",
    "Constraints:",
    "  2 <= nums.length <= 10^4",
    "  Only one valid answer exists.",
  ];
  lines.forEach((l, i) => ctx.fillText(l, left + 28 * s, 226 * s + i * 32 * s));
  // right: editor
  const ex = 764 * s;
  ctx.fillStyle = "#1e1e1e";
  ctx.fillRect(ex, 84 * s, w - ex - 24 * s, h - 104 * s);
  ctx.fillStyle = "#2d2d2d";
  ctx.fillRect(ex, 84 * s, w - ex - 24 * s, 44 * s);
  ctx.fillStyle = "#cfcfcf";
  ctx.font = `${16 * s}px Segoe UI, sans-serif`;
  ctx.fillText("Python3", ex + 20 * s, 112 * s);
  ctx.font = `${18 * s}px Cascadia Code, Consolas, monospace`;
  const code: [string, string][] = [
    ["#c586c0", "class Solution:"],
    ["#dcdcaa", "    def twoSum(self, nums: List[int], target: int) -> List[int]:"],
    ["#6a9955", "        # your code here"],
    ["#9cdcfe", "        "],
  ];
  code.forEach(([c, t], i) => {
    ctx.fillStyle = "#6e7681";
    ctx.fillText(String(i + 1), ex + 18 * s, 170 * s + i * 30 * s);
    ctx.fillStyle = c;
    ctx.fillText(t, ex + 56 * s, 170 * s + i * 30 * s);
  });
}

export interface FakeShot {
  dataUrl: string;
  thumbDataUrl: string;
  width: number;
  height: number;
}

export function makeFakeScreenshot(region = false): FakeShot {
  const width = region ? 760 : 1568;
  const height = region ? 520 : 882;
  if (typeof document === "undefined") {
    return { dataUrl: "", thumbDataUrl: "", width, height };
  }
  const full = document.createElement("canvas");
  full.width = 1568;
  full.height = 882;
  const fctx = full.getContext("2d");
  if (!fctx) return { dataUrl: "", thumbDataUrl: "", width, height };
  drawScreen(fctx, 1568, 882);

  const out = document.createElement("canvas");
  out.width = width;
  out.height = height;
  const octx = out.getContext("2d")!;
  if (region) octx.drawImage(full, 24, 84, 760, 520, 0, 0, 760, 520);
  else octx.drawImage(full, 0, 0);

  const thumb = document.createElement("canvas");
  const tw = 320;
  const th = Math.round((height / width) * tw);
  thumb.width = tw;
  thumb.height = th;
  thumb.getContext("2d")!.drawImage(out, 0, 0, tw, th);
  return {
    dataUrl: out.toDataURL("image/jpeg", 0.82),
    thumbDataUrl: thumb.toDataURL("image/jpeg", 0.8),
    width,
    height,
  };
}
