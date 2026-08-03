"""托盘菜单自动化（Win11 UIA 定位 + 真实鼠标点击）。
用法: python scripts/tray_click_test.py [目标项子串] [left|right]
以 /api/heartbeat 的 enabled 变化验证点击链路。
"""
import sys
import time
import ctypes

import uiautomation as auto

user32 = ctypes.windll.user32
MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP = 0x0002, 0x0004
MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP = 0x0008, 0x0010


def walk(ctrl, max_depth=24):
    stack = [(ctrl, 0)]
    while stack:
        c, d = stack.pop()
        if d > max_depth:
            continue
        yield c
        try:
            kids = c.GetChildren()
        except Exception:
            kids = []
        for k in reversed(kids):
            stack.append((k, d + 1))


def find_by_name(name_part, kinds, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for c in walk(auto.GetRootControl()):
            if c.ControlTypeName not in kinds:
                continue
            try:
                n = c.Name or ''
            except Exception:
                continue
            if name_part in n:
                return c
        time.sleep(0.4)
    return None


def real_click(x, y, button='left'):
    user32.SetCursorPos(int(x), int(y))
    time.sleep(0.15)
    if button == 'right':
        user32.mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0)
        time.sleep(0.08)
        user32.mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0)
    else:
        user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
        time.sleep(0.08)
        user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def main():
    target = sys.argv[1] if len(sys.argv) > 1 else '心跳同步'
    icon = find_by_name('知识库', ('ButtonControl', 'ListItemControl', 'Control', 'PaneControl'))
    if not icon:
        print('FAIL: 未找到托盘图标')
        return 1
    r = icon.BoundingRectangle
    cx, cy = (r.left + r.right) // 2, (r.top + r.bottom) // 2
    print(f'托盘图标中心: ({cx},{cy})')
    real_click(cx, cy, 'right')
    time.sleep(0.8)
    item = find_by_name(target, ('MenuItemControl',))
    if not item:
        print('FAIL: 菜单里没找到目标项')
        return 1
    r = item.BoundingRectangle
    ix, iy = (r.left + r.right) // 2, (r.top + r.bottom) // 2
    print(f'菜单项 {item.Name!r} 中心: ({ix},{iy})')
    real_click(ix, iy, 'left')
    time.sleep(1.0)
    print('OK: 真实鼠标已点击', item.Name)
    return 0


if __name__ == '__main__':
    auto.UIAutomationInitializerInThread()
    sys.exit(main())
