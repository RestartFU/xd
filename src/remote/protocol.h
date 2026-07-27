#pragma once

/*
 * Remote images are carried inline in the authenticated JSON request.
 *
 * The limits keep one paste from turning into an unbounded allocation on
 * either device while leaving plenty of room for full-resolution screenshots.
 */
#define XD_REMOTE_MAX_IMAGES 4
#define XD_REMOTE_MAX_IMAGE_BYTES (10 * 1024 * 1024)
#define XD_REMOTE_MAX_IMAGES_BYTES (20 * 1024 * 1024)
