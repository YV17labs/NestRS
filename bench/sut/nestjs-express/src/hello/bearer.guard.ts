import { CanActivate, ExecutionContext, Injectable } from '@nestjs/common';

@Injectable()
export class BearerGuard implements CanActivate {
  canActivate(context: ExecutionContext): boolean {
    const request = context
      .switchToHttp()
      .getRequest<{ headers: Record<string, string | string[] | undefined> }>();
    return request.headers.authorization === 'Bearer bench';
  }
}
