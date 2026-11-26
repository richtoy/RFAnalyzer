/* Desktop stand-in for Android's log header (librtlsdr from the Android app logs through it). */
#pragma once
#include <stdio.h>
#define ANDROID_LOG_WARN 5
#define __android_log_print(prio, tag, ...) (fprintf(stderr, "%s: ", tag), fprintf(stderr, __VA_ARGS__), fprintf(stderr, "\n"))
