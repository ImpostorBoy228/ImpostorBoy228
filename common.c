// common.c - shared ITS time system implementation
#include "common.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

// ------------------------------------------------------------------
// Earth Orientation Parameters (IERS finals.all) + cubic spline
// ------------------------------------------------------------------
static double *mjd_arr = NULL;
static double *dut1_arr = NULL;
static int n_eop = 0;
static double *second_deriv = NULL;   // for cubic spline
#define DEG (M_PI / 180.0)
static double epsilon = 23.4393*DEG;

Vec3 getSun(double mjd) {
    double n = mjd - 51544.5;
    double L = fmod(280.460 + 0.9856474 * n, 360.0) * DEG;
    double g_deg = fmod(357.528 + 0.9856003 * n, 360.0);
    double g = g_deg * DEG;
    double lambda = L
              + (1.915 * DEG) * sin(g)
              + (0.020 * DEG) * sin(2 * g);

    return (Vec3){cos(lambda), cos(epsilon)*sin(lambda), sin(epsilon)*sin(lambda)};
}

Vec3 getNsk(double mjd) {
    double n = mjd - 51544.5;
    double phi = LAT * DEG;
    double lambda = LON * DEG;
    double theta = (280.46061837 + 360.98564736629 * n) * DEG;
    double x = cos(phi) * cos(lambda + theta);
    double y = cos(phi) * sin(lambda + theta);
    double z = sin(phi);
    return (Vec3){x, y, z};
}

double mjd_from_unix(time_t t) {
    return (double)t / SECS_PER_DAY + 40587.0;   // 1970-01-01 = MJD 40587
}

void load_finals(const char *filename) {
    FILE *f = fopen(filename, "r");
    if (!f) { perror("finals.all"); exit(1); }
    char line[256];
    // first pass: count valid lines
    while (fgets(line, sizeof(line), f)) {
        if (strlen(line) < 68) continue;
        if (strncmp(line, "MJD", 3) == 0) continue;
        n_eop++;
    }
    rewind(f);
    mjd_arr = malloc(n_eop * sizeof(double));
    dut1_arr = malloc(n_eop * sizeof(double));
    if (!mjd_arr || !dut1_arr) { fprintf(stderr, "malloc failed\n"); exit(1); }
    int i = 0;
    while (fgets(line, sizeof(line), f)) {
        if (strlen(line) < 68) continue;
        if (strncmp(line, "MJD", 3) == 0) continue;
        char mjd_str[10], dut1_str[12];
        strncpy(mjd_str, line + 7, 8); mjd_str[8] = '\0';
        strncpy(dut1_str, line + 57, 11); dut1_str[11] = '\0';
        double mjd = atof(mjd_str);
        double dut1 = atof(dut1_str);
        if (mjd > 0 && dut1 > -10 && dut1 < 10) {
            mjd_arr[i] = mjd;
            dut1_arr[i] = dut1;
            i++;
        }
    }
    n_eop = i;
    fclose(f);
}

/* Build natural cubic spline coefficients (second derivatives) */
void build_spline(void) {
    if (n_eop < 2) {
        second_deriv = NULL;
        return;
    }
    second_deriv = malloc(n_eop * sizeof(double));
    int n = n_eop;
    double *h = malloc((n-1) * sizeof(double));
    double *b = malloc((n-1) * sizeof(double));
    for (int i=0; i<n-1; i++) {
        h[i] = mjd_arr[i+1] - mjd_arr[i];
        b[i] = (dut1_arr[i+1] - dut1_arr[i]) / h[i];
    }
    // Tridiagonal system: d[i]*s''[i] + l[i]*s''[i-1] + mu[i]*s''[i+1] = z[i]
    double *d = malloc(n * sizeof(double));
    double *l = malloc(n * sizeof(double));
    double *mu = malloc(n * sizeof(double));
    double *z = malloc(n * sizeof(double));
    d[0] = 1.0; l[0] = 0.0; mu[0] = 0.0; z[0] = 0.0;
    for (int i=1; i<n-1; i++) {
        double h_im1 = h[i-1];
        double h_i = h[i];
        l[i] = h_im1 / (h_im1 + h_i);
        mu[i] = h_i / (h_im1 + h_i);
        d[i] = 2.0;
        z[i] = 6.0 * ((b[i] - b[i-1]) / (h_im1 + h_i));
    }
    d[n-1] = 1.0; l[n-1] = 0.0; mu[n-1] = 0.0; z[n-1] = 0.0;
    // Forward sweep
    for (int i=1; i<n; i++) {
        double factor = l[i] / d[i-1];
        d[i] -= factor * mu[i-1];
        z[i] -= factor * z[i-1];
    }
    // Back substitution
    second_deriv[n-1] = z[n-1] / d[n-1];
    for (int i=n-2; i>=0; i--) {
        second_deriv[i] = (z[i] - mu[i] * second_deriv[i+1]) / d[i];
    }
    free(h); free(b); free(d); free(l); free(mu); free(z);
}

/* Interpolate dUT1 at given MJD using cubic spline */
double interpolate_dut1_spline(double mjd) {
    if (n_eop == 0) return 0.0;
    if (mjd <= mjd_arr[0]) return dut1_arr[0];
    if (mjd >= mjd_arr[n_eop-1]) return dut1_arr[n_eop-1];
    int i = 0;
    while (i < n_eop-1 && mjd_arr[i+1] < mjd) i++;
    double h = mjd_arr[i+1] - mjd_arr[i];
    double t = (mjd - mjd_arr[i]) / h;
    double y0 = dut1_arr[i], y1 = dut1_arr[i+1];
    if (!second_deriv)
        return (1-t)*y0 + t*y1;
    double s0 = second_deriv[i], s1 = second_deriv[i+1];
    return (1-t)*y0 + t*y1 + (t*t*t - t)*((1-t)*s0 + t*s1)*h*h/6.0;
}

// Astronomy: Julian Date, Sun position, hour angle, twilight times
double jdn(int y, int m, int d) {
    if (m <= 2) { y--; m += 12; }
    int A = y / 100;
    int B = 2 - A + A / 4;
    return floor(365.25 * (y + 4716)) + floor(30.6001 * (m + 1)) + d + B - 1524.5;
}

void sun_position(double jd, double *decl, double *eq_time) {
    double T = (jd - 2451545.0) / 36525.0;
    double L0 = fmod(280.46646 + 36000.76983 * T + 0.0003032 * T * T, 360.0);
    double M = fmod(357.52911 + 35999.05029 * T - 0.0001537 * T * T, 360.0);
    double C = (1.914602 - 0.004817 * T - 0.000014 * T * T) * sin(M * M_PI / 180.0)
             + (0.019993 - 0.000101 * T) * sin(2 * M * M_PI / 180.0)
             + 0.000289 * sin(3 * M * M_PI / 180.0);
    double sun_lon = L0 + C;
    double obliq = 23.439291 - 0.0130042 * T;
    double alpha = atan2(cos(obliq * M_PI / 180.0) * sin(sun_lon * M_PI / 180.0),
                         cos(sun_lon * M_PI / 180.0)) * 180.0 / M_PI;
    alpha = fmod(alpha + 360.0, 360.0);
    double delta = asin(sin(obliq * M_PI / 180.0) * sin(sun_lon * M_PI / 180.0)) * 180.0 / M_PI;
    *decl = delta;
    double E = L0 - alpha;
    if (E < -180.0) E += 360.0;
    if (E > 180.0) E -= 360.0;
    *eq_time = E * 4.0;
}

// Returns the hour angle in hours. Returns -1.0 when the sun never reaches
// the given zenith altitude at this latitude (polar day / polar night), so
// the caller can detect that no such event occurs.
double hour_angle(double lat, double decl, double zenith, int sign) {
    double cos_ha = (cos(zenith * M_PI / 180.0) - sin(lat * M_PI / 180.0) * sin(decl * M_PI / 180.0)) /
                    (cos(lat * M_PI / 180.0) * cos(decl * M_PI / 180.0));
    if (cos_ha < -1.0 || cos_ha > 1.0) return -1.0;
    double ha = acos(cos_ha) * 180.0 / M_PI / 15.0;
    return sign * ha;
}

void compute_times(int y, int m, int d, double *sunset, double *twilight_end, double *daylen, int *has_night) {
    double jd = jdn(y, m, d) - 0.5;
    double decl, eq_time;
    sun_position(jd, &decl, &eq_time);
    double noon = 12.0 - LON / 15.0 - eq_time / 60.0;
    double ha_sunset = hour_angle(LAT, decl, 90.833, 1);
    double ha_twilight = hour_angle(LAT, decl, ZENITH, 1);
    *has_night = (ha_twilight > 0.0);
    if (!*has_night) {
        *sunset = -1.0; *twilight_end = -1.0; *daylen = 0.0;
        return;
    }
    double set = noon + ha_sunset;
    double twi = noon + ha_twilight;
    if (set < 0.0) set += 24.0;
    if (twi < 0.0) twi += 24.0;
    if (set >= 24.0) set -= 24.0;
    if (twi >= 24.0) twi -= 24.0;
    *sunset = set * 3600.0;
    *twilight_end = twi * 3600.0;
    *daylen = 2.0 * ha_sunset;
}

// offset computation (UT1-based): the earliest astronomical nightfall over
// the 50-year window 1976-2026, expressed in UT1 seconds of the day.
double compute_earliest_night(int *out_y, int *out_m, int *out_d) {
    double min_twilight = 1e9;
    int best_y = 0, best_m = 0, best_d = 0;
    for (int y = 1976; y < 2026; y++) {
        for (int m = 1; m <= 12; m++) {
            int dim;
            if (m == 2) dim = (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) ? 29 : 28;
            else if (m == 4 || m == 6 || m == 9 || m == 11) dim = 30;
            else dim = 31;
            for (int d = 1; d <= dim; d++) {
                double sunset, twilight, daylen;
                int has_night;
                compute_times(y, m, d, &sunset, &twilight, &daylen, &has_night);
                if (!has_night) continue;
                if (twilight < min_twilight) {
                    min_twilight = twilight;
                    best_y = y; best_m = m; best_d = d;
                }
            }
        }
    }
    if (out_y) *out_y = best_y;
    if (out_m) *out_m = best_m;
    if (out_d) *out_d = best_d;
    return (min_twilight < 1e9) ? min_twilight : -1.0;
}

double compute_offset(void) {
    return compute_earliest_night(NULL, NULL, NULL);
}

double get_offset(void) {
    FILE *f = fopen(OFFSET_FILE, "r");
    double offset;
    if (f && fscanf(f, "%lf", &offset) == 1) { fclose(f); return offset; }
    if (f) fclose(f);
    // offset.dat missing: ask the dedicated binary to (re)generate it.
    int rc = system("its-offset 2>/dev/null || ./its-offset 2>/dev/null");
    (void)rc;
    f = fopen(OFFSET_FILE, "r");
    if (f && fscanf(f, "%lf", &offset) == 1) { fclose(f); return offset; }
    if (f) fclose(f);
    fprintf(stderr, "Offset computation fuckup\n");
    exit(1);
}

// formatting helpers
void format_time(double sec, char *buf, size_t size) {
    int h = (int)(sec / 3600);
    int m = (int)(fmod(sec, 3600) / 60);
    int s = (int)(fmod(sec, 60));
    snprintf(buf, size, "%02d:%02d:%02d", h, m, s);
}

void format_time_j(double sec, char *buf, size_t size) {
    int h = (int)(sec / 3600);
    int m = (int)(fmod(sec, 3600) / 60);
    int s = (int)(fmod(sec, 60));
    snprintf(buf, size, "%02d時%02d分%02d秒", h, m, s);
}

void free_eop(void) {
    free(mjd_arr);
    free(dut1_arr);
    free(second_deriv);
    mjd_arr = NULL;
    dut1_arr = NULL;
    second_deriv = NULL;
}
