const units = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'];

export function size(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = unit === 0 || value >= 10 ? 0 : 1;
  return `${value.toFixed(digits).replace('.', ',')} ${units[unit]}`;
}

const dateFormat = new Intl.DateTimeFormat('ru', { dateStyle: 'medium', timeStyle: 'short' });

export function date(iso: string): string {
  return dateFormat.format(new Date(iso));
}
