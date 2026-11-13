import { cn } from '../../lib/utils'
import { buttonVariants } from './button'

export const inputClass = cn(
  'rounded-md border border-input bg-background px-3 py-1.5 text-sm shadow-xs',
  'placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
)

export const outlineButton = buttonVariants({ variant: 'outline', size: 'sm' })
export const primaryButton = buttonVariants({ size: 'sm' })
export const dangerButton = buttonVariants({ variant: 'destructive', size: 'sm' })
