#ifndef COMMON_H
#define COMMON_H

#include <time.h>
#include <stdint.h>

/* ------------------------------------------------------------------
   ITS TIME SYSTEM – SPECIFICATIONS
   ------------------------------------------------------------------
   The ITS time system defines a custom time scale based on the
   astronomical "start of the day" in Novosibirsk, Russia
   (latitude 55.03 N, longitude 82.93 E). The epoch is
   2026-06-22 00:00:00 UTC, but the zero point of the ITS time is
   not UTC midnight - it is the earliest astronomical nightfall
   (twilight end, when the Sun is 108 below the horizon) observed
   in Novosibirsk over the 50-year window from 1976 to 2026.

   ITS time is stored as a signed integer of nanoseconds elapsed
   since that epoch (the UT1 instant defined above). It can be
   positive (future) or negative (past).

   The calculation uses UT1 (Universal Time), which accounts for
   the irregular rotation of the Earth. The difference UT1-UTC
   (called DUT1) is obtained from the IERS `finals.all` file. Cubic
   spline interpolation is applied between daily values to get DUT1
   at arbitrary moments with high precision.

   Calendar:
   - 1 ITS year  = 147 days
   - 1 ITS month =  21 days
   - 1 ITS day   = 86400 seconds (SI seconds, but counted in UT1)
   A year = 7 months. Months/years are numbered from 0.
   ------------------------------------------------------------------ */

typedef struct { double x, y, z; } Vec3;

// Sun and observer position vectors (unit vectors, equatorial frame)
Vec3 getSun(double mjd);
Vec3 getNsk(double mjd);

#define LAT 55.03
#define LON 82.93            // Novosibirsk
#define ZENITH 108.0
#define SECS_PER_DAY 86400.0
#define OFFSET_FILE "offset.dat"
#define FINALS_FILE "finals.all"
#define EPOCH_UNIX 1782086400.0   // 2026-06-22 00:00:00 UTC

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

#define ITS_DAY_NS 86400000000000ULL   // 86400 * 1e9
#define ITS_YEAR_DAYS 147
#define ITS_MONTH_DAYS 21

// Earth Orientation Parameter state (populated by load_finals)
void load_finals(const char *filename);
void build_spline(void);
double interpolate_dut1_spline(double mjd);
double mjd_from_unix(time_t t);

// astronomical helpers
double jdn(int y, int m, int d);
void sun_position(double jd, double *decl, double *eq_time);
double hour_angle(double lat, double decl, double zenith, int sign);
void compute_times(int y, int m, int d, double *sunset, double *twilight_end, double *daylen, int *has_night);
double compute_offset(void);
// scan the 1976-2026 window and return the UT1 seconds-of-day of the earliest
// astronomical nightfall, filling the Gregorian date it occurs on (may be NULL).
double compute_earliest_night(int *out_y, int *out_m, int *out_d);

// offset from UTC midnight to the earliest nightfall (UT1 seconds of day).
// Reads offset.dat; if missing, invokes the its-offset binary to generate
// it, then reads the result.
double get_offset(void);

// time formatting
void format_time(double sec, char *buf, size_t size);
void format_time_j(double sec, char *buf, size_t size);

// release Earth Orientation Parameter state
void free_eop(void);

#endif
