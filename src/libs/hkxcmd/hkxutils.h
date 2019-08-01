#pragma once
#ifndef _WINDOWS_
#include <windows.h>
#endif
#include <tchar.h>
#include <string>
#include <map>
#include <vector>
#include <list>
#include <map>

#include <cstring>
#include <cctype>
#include <clocale>
#include <malloc.h>
#include <sstream>

#define _USE_MATH_DEFINES
#include <cmath>

typedef struct EnumLookupType {
    int value;
    const char *name;
} EnumLookupType;
