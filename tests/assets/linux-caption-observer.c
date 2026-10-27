#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int property(Display *display, Window window, const char *name,
                    Atom type, int format, unsigned long count, unsigned char **value,
                    unsigned long *items) {
  Atom actual_type;
  int actual_format;
  unsigned long remaining;
  return XGetWindowProperty(display, window, XInternAtom(display, name, False),
    0, count, False, type, &actual_type, &actual_format, items, &remaining, value) == Success
    && actual_type == type && actual_format == format && remaining == 0;
}

int main(int argc, char **argv) {
  if (argc != 2) return 77;
  errno = 0;
  char *end = NULL;
  unsigned long parsed = strtoul(argv[1], &end, 0);
  if (errno || !end || *end || !parsed) return 77;

  Display *display = XOpenDisplay(NULL);
  if (!display) return 77;
  Window root = DefaultRootWindow(display);
  unsigned char *value = NULL;
  unsigned long items = 0;
  if (!property(display, root, "_NET_SUPPORTING_WM_CHECK", XA_WINDOW, 32, 1, &value, &items)
      || items != 1) {
    if (value) XFree(value);
    XCloseDisplay(display);
    return 77;
  }
  Window wm = *(Window *)value;
  XFree(value);
  value = NULL;
  if (!property(display, wm, "_NET_SUPPORTING_WM_CHECK", XA_WINDOW, 32, 1, &value, &items)
      || items != 1 || *(Window *)value != wm) {
    if (value) XFree(value);
    XCloseDisplay(display);
    return 77;
  }
  XFree(value);
  value = NULL;
  Atom utf8 = XInternAtom(display, "UTF8_STRING", False);
  if (!property(display, wm, "_NET_WM_NAME", utf8, 8, 16, &value, &items)
      || items != 5 || memcmp(value, "Xfwm4", 5) != 0) {
    if (value) XFree(value);
    XCloseDisplay(display);
    return 77;
  }
  XFree(value);

  Window client = (Window)parsed;
  Window client_root, frame;
  Window *children = NULL;
  unsigned int child_count = 0;
  XWindowAttributes client_attributes, frame_attributes;
  if (!XGetWindowAttributes(display, client, &client_attributes)
      || client_attributes.map_state != IsViewable
      || !XQueryTree(display, client, &client_root, &frame, &children, &child_count)
      || client_root != root || frame == root) {
    if (children) XFree(children);
    XCloseDisplay(display);
    return 77;
  }
  if (children) XFree(children);
  children = NULL;
  if (!XGetWindowAttributes(display, frame, &frame_attributes)
      || frame_attributes.map_state != IsViewable) {
    XCloseDisplay(display);
    return 77;
  }
  Window frame_root, frame_parent;
  if (!XQueryTree(display, frame, &frame_root, &frame_parent, &children, &child_count)
      || frame_root != root || frame_parent != root) {
    if (children) XFree(children);
    XCloseDisplay(display);
    return 77;
  }

  int contains_client = 0;
  unsigned int caption_controls = 0;
  for (unsigned int i = 0; i < child_count; ++i) {
    Window child = children[i];
    if (child == client) {
      contains_client = 1;
      continue;
    }
    XWindowAttributes attributes;
    if (!XGetWindowAttributes(display, child, &attributes)) {
      XFree(children);
      XCloseDisplay(display);
      return 77;
    }
    // Xfwm selects Enter/Leave on caption buttons; title and frame edges
    // use NoEventMask. Button presses are handled by the parent frame.
    const long button_events = EnterWindowMask | LeaveWindowMask;
    if (attributes.map_state == IsViewable
        && (attributes.all_event_masks & button_events) == button_events
        && attributes.x >= 0 && attributes.y >= 0
        && attributes.width > 0 && attributes.height > 0
        && attributes.x + attributes.width <= frame_attributes.width
        && attributes.y + attributes.height <= 64) {
      ++caption_controls;
    }
  }
  XFree(children);
  XCloseDisplay(display);
  if (!contains_client || caption_controls > 1) return 77;
  printf("%u\n", caption_controls);
  return 0;
}
